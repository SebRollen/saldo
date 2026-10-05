//! Resolves the names in a model: what each name in each module stands for,
//! which accounts exist, and which `let`s are values and which are sets of
//! days. Produces a [`Model`] in which every name is spelled out in full.

use crate::ast::schedule::{self, DaySet, Dow, Month, quarter_from_name};
use crate::ast::{
    AccountKind, AggKind, BinOp, Decl, EntryStmt, Expr, Growth, LetBody, ModBody, Move, Path,
    Program, Schedule, Span, Spanned, SpannedExpr, Stmt, TimeUnit,
};
use crate::errors::Diagnostic;
use crate::eval::BUILTINS;
use chrono::NaiveDate;
use indexmap::IndexMap;
use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};

#[derive(Debug, Clone)]
pub struct Account {
    /// Its opening balance, read as its kind reads, and the day it opens.
    pub opening: Option<(SpannedExpr, NaiveDate)>,
}

#[derive(Debug)]
pub struct EntryDef {
    /// Its full name, like `loans.car`.
    pub name: String,
    pub label: String,
    pub schedule: Schedule,
    pub body: Vec<EntryStmt>,
    /// The names of the tags on its moves.
    pub tags: Vec<String>,
    /// The entries it fires after, by full name, and where they're named.
    pub after: Vec<(String, Span)>,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct ArmDef {
    pub when: DaySet,
    pub value: SpannedExpr,
    pub growth: Option<Growth>,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub enum ParamBody {
    Const(SpannedExpr),
    /// Arms, of which the first that covers a day gives the value.
    Arms(Vec<ArmDef>),
}

#[derive(Debug, Clone)]
pub struct FnDef {
    pub params: Vec<String>,
    pub body: Vec<Stmt>,
    pub span: Span,
}

#[derive(Debug)]
pub struct Model {
    /// The accounts money moves into or out of, ordered by path.
    pub accounts: IndexMap<Path, Account>,
    /// Values by full name, each after the values it reads.
    pub params: IndexMap<String, ParamBody>,
    /// Functions by full name, each after the functions it calls.
    pub fns: IndexMap<String, FnDef>,
    pub entries: Vec<EntryDef>,
    pub checks: Vec<(Schedule, SpannedExpr)>,
    /// Problems that don't stop the model from running.
    pub warnings: Vec<Diagnostic>,
}

pub fn resolve(program: &Program) -> Result<Model, Vec<Diagnostic>> {
    let mut resolver = Resolver::default();
    resolver.collect(&[], &program.decls);
    resolver.classify_lets();
    resolver.resolve_all();
    if !resolver.diags.is_empty() {
        // A set of days with an error is reported where it's declared and
        // again wherever it's used.
        resolver
            .diags
            .sort_by(|a, b| (a.span.start, &a.message).cmp(&(b.span.start, &b.message)));
        resolver
            .diags
            .dedup_by(|a, b| a.span == b.span && a.message == b.message);
        return Err(resolver.diags);
    }
    let model = resolver.into_model();
    // Types are only meaningful once every name resolves.
    let diags = crate::typecheck::check(&model);
    if diags.is_empty() {
        Ok(model)
    } else {
        Err(diags)
    }
}

/// What a name declared in a module is.
#[derive(Clone, Debug)]
enum Symbol {
    /// A `let`, by its index in `Resolver::lets`.
    Let(usize),
    Fn(String),
    /// An entry by full name, and the tags on its moves.
    Entry(String, HashSet<String>),
    Mod(Vec<String>),
}

/// For each pair of entries where the first fires before the second, why:
/// where the second reads something, what it is, and where the first moves
/// it, or `None` if the second names the first with `after`.
type Before = BTreeMap<(usize, usize), Option<(Span, String, Span)>>;

/// Something an entry reads that another entry can change.
enum Read {
    Account(Path),
    /// A tag, by its entry's index and its name.
    Tag(usize, String),
}

/// What a path names.
enum Target {
    Let(usize),
    Fn(String),
    Entry(String),
    Mod,
    Tag(String, String),
    Account(Path),
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Class {
    Value,
    Days,
}

struct LetDecl<'a> {
    module: Vec<String>,
    full: String,
    body: &'a LetBody,
    span: Span,
}

struct FnDecl<'a> {
    module: Vec<String>,
    full: String,
    params: &'a [Spanned<String>],
    body: &'a [Stmt],
    span: Span,
}

struct EntryDecl<'a> {
    module: Vec<String>,
    full: String,
    label: Option<&'a String>,
    when: Option<&'a SpannedExpr>,
    after: &'a [Spanned<Path>],
    body: &'a [EntryStmt],
    tags: HashSet<String>,
    span: Span,
}

/// An account and its opening balance, in a `balances` block.
type OpeningBalance = (Spanned<Path>, SpannedExpr);

/// Where the names in an expression resolve.
struct Scope<'s> {
    module: &'s [String],
    /// Function parameters and `let`s in scope.
    locals: &'s [String],
    /// The entry being resolved, and its tags.
    entry: Option<(&'s str, &'s HashSet<String>)>,
    /// The function being resolved, which can't read accounts or totals.
    function: Option<&'s str>,
}

#[derive(Default)]
struct Resolver<'a> {
    diags: Vec<Diagnostic>,
    /// The names declared in each module, by module path.
    namespaces: HashMap<Vec<String>, HashMap<String, (Symbol, Span)>>,
    lets: Vec<LetDecl<'a>>,
    classes: Vec<Option<Class>>,
    fn_decls: Vec<FnDecl<'a>>,
    entry_decls: Vec<EntryDecl<'a>>,
    check_decls: Vec<(Vec<String>, &'a SpannedExpr, Option<&'a SpannedExpr>)>,
    balance_decls: Vec<(Vec<String>, NaiveDate, &'a [OpeningBalance])>,

    /// Accounts money moves into or out of, and where each is first named.
    leaves: BTreeMap<Path, Span>,
    /// Accounts and groups read, and where.
    reads: Vec<(Path, Span)>,

    accounts: BTreeMap<Path, Account>,
    params: HashMap<String, ParamBody>,
    param_spans: HashMap<String, Span>,
    fns: HashMap<String, FnDef>,
    entries: Vec<EntryDef>,
    checks: Vec<(Schedule, SpannedExpr)>,
    warnings: Vec<Diagnostic>,
}

/// Names that can't name a value, a function, an entry or a module.
fn reserved(name: &str) -> Option<&'static str> {
    if AccountKind::from_root(name).is_some() {
        return Some("starts the names of accounts");
    }
    if Dow::from_name(name).is_some()
        || Month::from_name(name).is_some()
        || quarter_from_name(name).is_some()
        || name == "last"
    {
        return Some("is a set of days");
    }
    if matches!(
        name,
        "_" | "and" | "or" | "not" | "if" | "then" | "else" | "compounding" | "adding" | "every"
    ) {
        return Some("is a keyword");
    }
    None
}

fn join(module: &[String], name: &str) -> String {
    module
        .iter()
        .map(String::as_str)
        .chain([name])
        .collect::<Vec<_>>()
        .join(".")
}

impl<'a> Resolver<'a> {
    // ---- collecting declarations ----

    fn collect(&mut self, module: &[String], decls: &'a [Spanned<Decl>]) {
        self.namespaces.entry(module.to_vec()).or_default();
        for (decl, span) in decls {
            let span = *span;
            match decl {
                Decl::Let { name, body } => {
                    let index = self.lets.len();
                    if self.declare(module, name, Symbol::Let(index)) {
                        self.lets.push(LetDecl {
                            module: module.to_vec(),
                            full: join(module, &name.0),
                            body,
                            span,
                        });
                    }
                }
                Decl::Fn { name, params, body } => {
                    let full = join(module, &name.0);
                    if BUILTINS.iter().any(|(builtin, _)| *builtin == name.0) {
                        self.diags.push(Diagnostic::new(
                            name.1,
                            format!("`{}` is a built-in function", name.0),
                        ));
                    } else if self.declare(module, name, Symbol::Fn(full.clone())) {
                        self.fn_decls.push(FnDecl {
                            module: module.to_vec(),
                            full,
                            params,
                            body,
                            span,
                        });
                    }
                }
                Decl::Entry {
                    name,
                    label,
                    when,
                    after,
                    body,
                } => {
                    let full = join(module, &name.0);
                    let tags: HashSet<String> = body
                        .iter()
                        .filter_map(|stmt| match stmt {
                            EntryStmt::Move(Move { tag: Some(tag), .. }) => Some(tag.0.clone()),
                            _ => None,
                        })
                        .collect();
                    if self.declare(module, name, Symbol::Entry(full.clone(), tags.clone())) {
                        self.entry_decls.push(EntryDecl {
                            module: module.to_vec(),
                            full,
                            label: label.as_ref(),
                            when: when.as_ref(),
                            after,
                            body,
                            tags,
                            span,
                        });
                    }
                }
                Decl::Check { expr, when } => {
                    self.check_decls
                        .push((module.to_vec(), expr, when.as_ref()));
                }
                Decl::Balances { date, balances } => {
                    self.balance_decls.push((module.to_vec(), *date, balances));
                }
                Decl::Mod { name, body } => {
                    let path: Vec<String> =
                        module.iter().cloned().chain([name.0.clone()]).collect();
                    if self.declare(module, name, Symbol::Mod(path.clone()))
                        && let ModBody::Inline(decls) = body
                    {
                        self.collect(&path, decls);
                    }
                }
            }
        }
    }

    /// Declares `name` in `module`, unless it can't be: returns whether it was.
    fn declare(
        &mut self,
        module: &[String],
        (name, span): &Spanned<String>,
        symbol: Symbol,
    ) -> bool {
        if let Some(why) = reserved(name) {
            self.diags.push(Diagnostic::new(
                *span,
                format!("`{name}` {why}, so it can't name anything else"),
            ));
            return false;
        }
        let namespace = self.namespaces.entry(module.to_vec()).or_default();
        if let Some((_, previous)) = namespace.get(name) {
            let previous = *previous;
            self.diags.push(
                Diagnostic::new(*span, format!("`{name}` is already declared"))
                    .with_note(previous, "declared here"),
            );
            return false;
        }
        namespace.insert(name.clone(), (symbol, *span));
        true
    }

    // ---- looking up names ----

    /// What `path` names in `module`: a name in the module, or else at the
    /// top of the model, and then names inside that.
    fn lookup(&self, module: &[String], path: &Path, span: Span) -> Result<Target, Diagnostic> {
        let first = &path.0[0];
        if AccountKind::from_root(first).is_some() {
            return Ok(Target::Account(path.clone()));
        }
        let found = self
            .namespaces
            .get(module)
            .and_then(|ns| ns.get(first))
            .or_else(|| {
                self.namespaces
                    .get(&Vec::new())
                    .and_then(|ns| ns.get(first))
            });
        let Some((mut symbol, _)) = found.cloned() else {
            let visible = self
                .namespaces
                .get(module)
                .into_iter()
                .chain(self.namespaces.get(&Vec::new()))
                .flat_map(|ns| ns.keys().map(String::as_str))
                .chain(AccountKind::ROOTS);
            let mut diag = Diagnostic::new(span, format!("unknown name `{first}`"));
            if let Some(close) = crate::util::closest(first, visible) {
                diag = diag.with_note(span, format!("did you mean `{close}`?"));
            }
            return Err(diag);
        };
        for (i, part) in path.0.iter().enumerate().skip(1) {
            let so_far = Path(path.0[..i].to_vec());
            match symbol {
                Symbol::Mod(m) => {
                    let Some((next, _)) = self.namespaces.get(&m).and_then(|ns| ns.get(part))
                    else {
                        return Err(Diagnostic::new(
                            span,
                            format!("module `{so_far}` has nothing called `{part}`"),
                        ));
                    };
                    symbol = next.clone();
                }
                Symbol::Entry(entry, tags) => {
                    if tags.contains(part) && i + 1 == path.0.len() {
                        return Ok(Target::Tag(entry, part.clone()));
                    }
                    return Err(Diagnostic::new(
                        span,
                        format!("entry `{so_far}` has no tag `{part}`"),
                    ));
                }
                Symbol::Let(_) | Symbol::Fn(_) => {
                    return Err(Diagnostic::new(
                        span,
                        format!("`{so_far}` has nothing inside it, so `{path}` names nothing"),
                    ));
                }
            }
        }
        Ok(match symbol {
            Symbol::Let(i) => Target::Let(i),
            Symbol::Fn(full) => Target::Fn(full),
            Symbol::Entry(full, _) => Target::Entry(full),
            Symbol::Mod(_) => Target::Mod,
        })
    }

    // ---- values and sets of days ----

    /// Works out which `let`s are sets of days.
    fn classify_lets(&mut self) {
        self.classes = vec![None; self.lets.len()];
        for i in 0..self.lets.len() {
            self.class_of(i, &mut Vec::new());
        }
    }

    fn class_of(&mut self, i: usize, seen: &mut Vec<usize>) -> Class {
        if let Some(class) = self.classes[i] {
            return class;
        }
        // A cycle is reported when the values are resolved.
        if seen.contains(&i) {
            return Class::Value;
        }
        seen.push(i);
        let class = match self.lets[i].body {
            LetBody::Arms(_) => Class::Value,
            LetBody::Expr(e) => {
                let module = self.lets[i].module.clone();
                self.class_of_expr(e, &module, seen)
            }
        };
        seen.pop();
        self.classes[i] = Some(class);
        class
    }

    fn class_of_expr(
        &mut self,
        (expr, span): &SpannedExpr,
        module: &[String],
        seen: &mut Vec<usize>,
    ) -> Class {
        match expr.as_ref() {
            Expr::Days(_) | Expr::Range { .. } | Expr::Bin(_, BinOp::Union | BinOp::Inter, _) => {
                Class::Days
            }
            Expr::Bin(a, BinOp::Sub | BinOp::Div, _) => self.class_of_expr(a, module, seen),
            Expr::Ref(path) => match self.lookup(module, path, *span) {
                Ok(Target::Let(i)) => self.class_of(i, seen),
                _ => Class::Value,
            },
            _ => Class::Value,
        }
    }

    /// The set of days `expr` describes, in `module`.
    fn lower_days(&self, expr: &SpannedExpr, module: &[String]) -> Result<DaySet, Diagnostic> {
        schedule::lower(expr, &mut |path, span| {
            self.named_days(module, path, span, &mut Vec::new())
        })
    }

    /// The set of days `path` names in `module`, as part of a larger one.
    /// `stack` holds the sets being read, to catch cycles.
    fn named_days(
        &self,
        module: &[String],
        path: &Path,
        span: Span,
        stack: &mut Vec<usize>,
    ) -> Result<DaySet, Diagnostic> {
        match self.lookup(module, path, span)? {
            Target::Let(i) if self.classes[i] == Some(Class::Days) => {
                if stack.contains(&i) {
                    return Err(Diagnostic::new(
                        span,
                        format!("`{}` is defined in terms of itself", self.lets[i].full),
                    ));
                }
                let LetBody::Expr(e) = self.lets[i].body else {
                    unreachable!("sets of days don't have arms")
                };
                stack.push(i);
                let module = &self.lets[i].module;
                let set = schedule::lower_part(e, &mut |path, span| {
                    self.named_days(module, path, span, stack)
                });
                stack.pop();
                set
            }
            Target::Let(_) => Err(Diagnostic::new(
                span,
                format!("`{path}` is a number, not a set of days"),
            )),
            _ => Err(Diagnostic::new(
                span,
                format!("`{path}` isn't a set of days"),
            )),
        }
    }

    fn schedule(&mut self, when: Option<&SpannedExpr>, module: &[String]) -> Option<Schedule> {
        let Some(when) = when else {
            return Some(Schedule::every_day());
        };
        match self.lower_days(when, module) {
            Ok(set) => Some(Schedule::new(set)),
            Err(diag) => {
                self.diags.push(diag);
                None
            }
        }
    }

    // ---- resolving everything ----

    fn resolve_all(&mut self) {
        for i in 0..self.lets.len() {
            self.resolve_let(i);
        }
        for i in 0..self.fn_decls.len() {
            self.resolve_fn(i);
        }
        for i in 0..self.entry_decls.len() {
            self.resolve_entry(i);
        }
        for i in 0..self.check_decls.len() {
            let (module, expr, when) = self.check_decls[i].clone();
            let scope = Scope {
                module: &module,
                locals: &[],
                entry: None,
                function: None,
            };
            let expr = self.resolve_expr(expr, &scope);
            if let Some(schedule) = self.schedule(when, &module) {
                self.checks.push((schedule, expr));
            }
        }
        for i in 0..self.balance_decls.len() {
            let (module, date, balances) = self.balance_decls[i].clone();
            for ((path, span), value) in balances {
                if !self.account_path(path, *span) {
                    continue;
                }
                let scope = Scope {
                    module: &module,
                    locals: &[],
                    entry: None,
                    function: None,
                };
                let value = self.resolve_expr(value, &scope);
                self.leaves.entry(path.clone()).or_insert(*span);
                let account = self
                    .accounts
                    .entry(path.clone())
                    .or_insert(Account { opening: None });
                if account.opening.is_some() {
                    self.diags.push(Diagnostic::new(
                        *span,
                        format!("`{path}` already has an opening balance"),
                    ));
                    continue;
                }
                account.opening = Some((value, date));
            }
        }
        self.check_param_cycles();
        self.check_values_read_no_state();
        validate_fn_calls(&self.fns, &mut self.diags);
        self.check_accounts();
        if self.diags.is_empty() {
            self.order_entries();
        }
    }

    fn resolve_let(&mut self, i: usize) {
        if self.classes[i] == Some(Class::Days) {
            // Sets of days are read where they're used; this reports their
            // errors even if they aren't used. A `/ n` can get the range it
            // counts from where it's used.
            if let LetBody::Expr(e) = self.lets[i].body {
                let module = &self.lets[i].module;
                let mut stack = vec![i];
                let set = schedule::lower_part(e, &mut |path, span| {
                    self.named_days(module, path, span, &mut stack)
                });
                if let Err(diag) = set {
                    self.diags.push(diag);
                }
            }
            return;
        }
        let module = self.lets[i].module.clone();
        let full = self.lets[i].full.clone();
        let scope = Scope {
            module: &module,
            locals: &[],
            entry: None,
            function: None,
        };
        let body = match self.lets[i].body {
            LetBody::Expr(e) => ParamBody::Const(self.resolve_expr(e, &scope)),
            LetBody::Arms(arms) => {
                let mut resolved: Vec<ArmDef> = Vec::new();
                for arm in arms {
                    let when = match &arm.when {
                        None => DaySet::All,
                        Some(when) => match self.lower_days(when, &module) {
                            Ok(set) => set,
                            Err(diag) => {
                                self.diags.push(diag);
                                continue;
                            }
                        },
                    };
                    let growth = arm.growth.as_ref().map(|g| Growth {
                        by: self.resolve_expr(&g.by, &scope),
                        ..g.clone()
                    });
                    if let Some(g) = &growth
                        && when.first_day().is_none()
                    {
                        self.diags.push(Diagnostic::new(
                            g.span,
                            format!(
                                "`{}` needs an arm with a start to count steps from, like `jul 2026.. =>`",
                                g.kind.keyword()
                            ),
                        ));
                    }
                    resolved.push(ArmDef {
                        when,
                        value: self.resolve_expr(&arm.value, &scope),
                        growth,
                        span: arm.span,
                    });
                }
                self.check_arms(&full, &resolved);
                ParamBody::Arms(resolved)
            }
        };
        self.param_spans.insert(full.clone(), self.lets[i].span);
        self.params.insert(full, body);
    }

    /// Reports arms that never apply because the arms above cover all their
    /// days.
    fn check_arms(&mut self, name: &str, arms: &[ArmDef]) {
        for (i, arm) in arms.iter().enumerate() {
            let above = &arms[..i];
            if let Some(otherwise) = above.iter().find(|a| a.when == DaySet::All) {
                self.diags.push(
                    Diagnostic::new(
                        arm.span,
                        format!("this arm of `{name}` comes after `_`, so it never applies"),
                    )
                    .with_note(otherwise.span, "`_` is here"),
                );
            } else if arm
                .when
                .is_shadowed_by(&above.iter().map(|a| &a.when).collect::<Vec<_>>())
                == Some(true)
            {
                self.diags.push(Diagnostic::new(
                    arm.span,
                    format!(
                        "this arm of `{name}` never applies: the arms above cover all its days"
                    ),
                ));
            }
        }
    }

    fn resolve_fn(&mut self, i: usize) {
        let decl = &self.fn_decls[i];
        let (module, full, span) = (decl.module.clone(), decl.full.clone(), decl.span);
        let (params, body) = (decl.params, decl.body);
        let mut locals: Vec<String> = Vec::new();
        for (param, param_span) in params {
            if locals.contains(param) {
                self.diags.push(Diagnostic::new(
                    *param_span,
                    format!("`{param}` is already a parameter of `{full}`"),
                ));
            }
            locals.push(param.clone());
        }
        let mut resolved = Vec::new();
        for stmt in body {
            let scope = Scope {
                module: &module,
                locals: &locals,
                entry: None,
                function: Some(full.as_str()),
            };
            match stmt {
                Stmt::Let { name, value } => {
                    let value = self.resolve_expr(value, &scope);
                    resolved.push(Stmt::Let {
                        name: name.clone(),
                        value,
                    });
                    locals.push(name.clone());
                }
                Stmt::Return(value) => {
                    resolved.push(Stmt::Return(self.resolve_expr(value, &scope)))
                }
            }
        }
        self.fns.insert(
            full,
            FnDef {
                params: params.iter().map(|(p, _)| p.clone()).collect(),
                body: resolved,
                span,
            },
        );
    }

    fn resolve_entry(&mut self, i: usize) {
        let decl = &self.entry_decls[i];
        let (module, full, span, when) =
            (decl.module.clone(), decl.full.clone(), decl.span, decl.when);
        let (label, body, tags) = (decl.label.cloned(), decl.body, decl.tags.clone());
        let mut after = Vec::new();
        for (path, path_span) in decl.after {
            match self.lookup(&module, path, *path_span) {
                Ok(Target::Entry(other)) if other == full => self.diags.push(Diagnostic::new(
                    *path_span,
                    "an entry can't fire after itself",
                )),
                Ok(Target::Entry(other)) => after.push((other, *path_span)),
                Ok(_) => self.diags.push(Diagnostic::new(
                    *path_span,
                    format!("`{path}` isn't an entry"),
                )),
                Err(diag) => self.diags.push(diag),
            }
        }
        let mut locals: Vec<String> = Vec::new();
        let mut resolved = Vec::new();
        for stmt in body {
            let scope = Scope {
                module: &module,
                locals: &locals,
                entry: Some((full.as_str(), &tags)),
                function: None,
            };
            match stmt {
                EntryStmt::Let { name, value } => {
                    let value = self.resolve_expr(value, &scope);
                    if locals.contains(&name.0) {
                        self.diags.push(Diagnostic::new(
                            name.1,
                            format!("`{}` is already a `let` in this entry", name.0),
                        ));
                    }
                    locals.push(name.0.clone());
                    resolved.push(EntryStmt::Let {
                        name: name.clone(),
                        value,
                    });
                }
                EntryStmt::Move(m) => {
                    let amount = self.resolve_expr(&m.amount, &scope);
                    for (path, path_span) in [&m.from, &m.to] {
                        if self.account_path(path, *path_span) {
                            self.leaves.entry(path.clone()).or_insert(*path_span);
                        }
                    }
                    resolved.push(EntryStmt::Move(Move {
                        amount,
                        ..m.clone()
                    }));
                }
            }
        }
        let Some(schedule) = self.schedule(when, &module) else {
            return;
        };
        let mut tags: Vec<String> = tags.into_iter().collect();
        tags.sort();
        self.entries.push(EntryDef {
            label: label.unwrap_or_else(|| full.clone()),
            name: full,
            schedule,
            body: resolved,
            tags,
            after,
            span,
        });
    }

    /// Checks that `path` can be an account that money moves into or out of,
    /// or that starts with a balance.
    fn account_path(&mut self, path: &Path, span: Span) -> bool {
        if path.account_kind().is_none() {
            let mut diag = Diagnostic::new(
                span,
                format!(
                    "`{path}` isn't an account: accounts start with `assets`, `liabilities`, \
                     `income`, `expenses` or `equity`"
                ),
            );
            if let Some(close) = crate::util::closest(&path.0[0], AccountKind::ROOTS) {
                diag = diag.with_note(span, format!("did you mean `{close}`?"));
            }
            self.diags.push(diag);
            return false;
        }
        if path.0.len() < 2 {
            self.diags.push(Diagnostic::new(
                span,
                format!("`{path}` holds every account of its kind; name one, like `{path}.cash`"),
            ));
            return false;
        }
        true
    }

    fn resolve_expr(&mut self, e: &SpannedExpr, scope: &Scope) -> SpannedExpr {
        let (expr, span) = e;
        let span = *span;
        let resolved = match expr.as_ref() {
            Expr::Num(_) | Expr::Bool(_) => return e.clone(),
            Expr::Ref(path) => match self.resolve_ref(path, span, scope) {
                Some(resolved) => resolved,
                None => return e.clone(),
            },
            Expr::Neg(x) => Expr::Neg(self.resolve_expr(x, scope)),
            Expr::Not(x) => Expr::Not(self.resolve_expr(x, scope)),
            Expr::Bin(_, op @ (BinOp::Union | BinOp::Inter), _) => {
                self.diags.push(Diagnostic::new(
                    span,
                    format!("`{op}` combines sets of days, but this needs a number"),
                ));
                return e.clone();
            }
            Expr::Bin(a, op, b) => Expr::Bin(
                self.resolve_expr(a, scope),
                *op,
                self.resolve_expr(b, scope),
            ),
            Expr::If { cond, then, else_ } => Expr::If {
                cond: self.resolve_expr(cond, scope),
                then: self.resolve_expr(then, scope),
                else_: self.resolve_expr(else_, scope),
            },
            Expr::Call(callee, args) => {
                let args: Vec<SpannedExpr> =
                    args.iter().map(|a| self.resolve_expr(a, scope)).collect();
                match self.resolve_callee(callee, args.len(), span, scope) {
                    Some(callee) => Expr::Call(callee, args),
                    None => return e.clone(),
                }
            }
            Expr::Per(x, unit) => Expr::Per(self.resolve_expr(x, scope), *unit),
            Expr::Period(x, unit) => match self.resolve_total(x, *unit, span, scope) {
                Some(resolved) => resolved,
                None => return e.clone(),
            },
            Expr::Days(_) | Expr::Range { .. } => {
                self.diags.push(Diagnostic::new(
                    span,
                    "this is a set of days, but this needs a number",
                ));
                return e.clone();
            }
            Expr::Global(_)
            | Expr::Local(_)
            | Expr::Balance(_)
            | Expr::TagTotal { .. }
            | Expr::AccountTotal(..) => return e.clone(),
        };
        (Box::new(resolved), span)
    }

    fn resolve_ref(&mut self, path: &Path, span: Span, scope: &Scope) -> Option<Expr> {
        if path.0.len() == 1 && scope.locals.contains(&path.0[0]) {
            return Some(Expr::Local(path.0[0].clone()));
        }
        let target = match self.lookup(scope.module, path, span) {
            Ok(target) => target,
            Err(diag) => {
                self.diags.push(diag);
                return None;
            }
        };
        let message = match target {
            Target::Account(path) => {
                if let Some(function) = scope.function {
                    format!(
                        "function `{function}` can't read balances; pass `{path}` in as an argument"
                    )
                } else {
                    self.reads.push((path.clone(), span));
                    return Some(Expr::Balance(path));
                }
            }
            Target::Let(i) if self.classes[i] == Some(Class::Days) => {
                format!("`{path}` is a set of days, but this needs a number")
            }
            Target::Let(i) => return Some(Expr::Global(self.lets[i].full.clone())),
            Target::Fn(_) => format!("`{path}` is a function; call it like `{path}(…)`"),
            Target::Entry(_) => format!(
                "`{path}` is an entry; read what its tagged moves moved with `{path}.<tag>[year]`"
            ),
            Target::Tag(..) => {
                format!("`{path}` is a tag; read what it moved so far with `{path}[year]`")
            }
            Target::Mod => format!("`{path}` is a module, not a value"),
        };
        self.diags.push(Diagnostic::new(span, message));
        None
    }

    fn resolve_callee(
        &mut self,
        callee: &Path,
        args: usize,
        span: Span,
        scope: &Scope,
    ) -> Option<Path> {
        let (name, arity) = if let [name] = callee.0.as_slice()
            && let Some((_, arity)) = BUILTINS.iter().find(|(builtin, _)| builtin == name)
        {
            (name.clone(), *arity)
        } else {
            match self.lookup(scope.module, callee, span) {
                Ok(Target::Fn(full)) => {
                    let decl = self
                        .fn_decls
                        .iter()
                        .find(|d| d.full == full)
                        .expect("declared functions are collected");
                    (full, decl.params.len())
                }
                Ok(_) => {
                    self.diags.push(Diagnostic::new(
                        span,
                        format!("`{callee}` isn't a function"),
                    ));
                    return None;
                }
                Err(mut diag) => {
                    diag.message = format!("unknown function `{callee}`");
                    self.diags.push(diag);
                    return None;
                }
            }
        };
        if args != arity {
            let word = if arity == 1 { "argument" } else { "arguments" };
            self.diags.push(Diagnostic::new(
                span,
                format!("`{callee}` takes {arity} {word}, got {args}"),
            ));
            return None;
        }
        Some(Path(vec![name]))
    }

    /// `x[unit]`: what a tag or an account has moved so far this period.
    fn resolve_total(
        &mut self,
        x: &SpannedExpr,
        unit: TimeUnit,
        span: Span,
        scope: &Scope,
    ) -> Option<Expr> {
        let Some(agg) = AggKind::from_unit(unit) else {
            self.diags.push(Diagnostic::new(
                span,
                format!("totals are kept for a year, quarter or month, not a {unit}"),
            ));
            return None;
        };
        if let Some(function) = scope.function {
            self.diags.push(Diagnostic::new(
                span,
                format!("function `{function}` can't read totals; pass them in as arguments"),
            ));
            return None;
        }
        let Expr::Ref(path) = x.0.as_ref() else {
            self.diags.push(Diagnostic::new(
                span,
                "only tags and accounts have totals, like `paycheck.contribution[year]` or \
                 `income.salary[year]`",
            ));
            return None;
        };
        if let ([tag], Some((entry, tags))) = (path.0.as_slice(), scope.entry)
            && tags.contains(tag)
        {
            return Some(Expr::TagTotal {
                entry: entry.to_string(),
                tag: tag.clone(),
                agg,
            });
        }
        match self.lookup(scope.module, path, x.1) {
            Ok(Target::Tag(entry, tag)) => Some(Expr::TagTotal { entry, tag, agg }),
            Ok(Target::Account(path)) => {
                self.reads.push((path.clone(), x.1));
                Some(Expr::AccountTotal(path, agg))
            }
            Ok(_) => {
                self.diags.push(Diagnostic::new(
                    x.1,
                    format!(
                        "`{path}` has no total; only tags and accounts do, like \
                         `paycheck.contribution[year]` or `income.salary[year]`"
                    ),
                ));
                None
            }
            Err(diag) => {
                self.diags.push(diag);
                None
            }
        }
    }

    // ---- checks across the model ----

    /// Checks that accounts are either accounts or groups of them, and that
    /// every account read exists.
    fn check_accounts(&mut self) {
        let leaves: Vec<(&Path, &Span)> = self.leaves.iter().collect();
        for (i, (leaf, span)) in leaves.iter().enumerate() {
            // Descendants sort right after their ancestors.
            if let Some((child, child_span)) = leaves.get(i + 1)
                && child.starts_with(leaf)
            {
                self.diags.push(
                    Diagnostic::new(
                        **span,
                        format!(
                            "`{leaf}` holds money, so it can't also be a group of accounts like `{child}`"
                        ),
                    )
                    .with_note(**child_span, format!("`{child}` is here")),
                );
            }
        }
        let groups: HashSet<Path> = self
            .leaves
            .keys()
            .flat_map(|leaf| (1..leaf.0.len()).map(|n| Path(leaf.0[..n].to_vec())))
            .collect();
        for (path, span) in &self.reads {
            if path.0.len() == 1 || self.leaves.contains_key(path) || groups.contains(path) {
                continue;
            }
            let names: Vec<String> = self.leaves.keys().chain(&groups).map(Path::join).collect();
            let mut diag = Diagnostic::new(
                *span,
                format!("no entry or balance moves money into or out of `{path}`"),
            );
            if let Some(close) =
                crate::util::closest(&path.join(), names.iter().map(String::as_str))
            {
                diag = diag.with_note(*span, format!("did you mean `{close}`?"));
            }
            self.diags.push(diag);
        }
        // Accounts next to each other with nearly the same name may be typos.
        let mut by_parent: BTreeMap<Path, Vec<(&Path, &Span)>> = BTreeMap::new();
        for (leaf, span) in &self.leaves {
            by_parent
                .entry(Path(leaf.0[..leaf.0.len() - 1].to_vec()))
                .or_default()
                .push((leaf, span));
        }
        for siblings in by_parent.values() {
            for (i, (a, _)) in siblings.iter().enumerate() {
                for (b, b_span) in &siblings[i + 1..] {
                    let (name_a, name_b) =
                        (a.0.last().expect("a name"), b.0.last().expect("a name"));
                    if name_a.chars().count() >= 4
                        && crate::util::edit_distance(name_a, name_b) == 1
                    {
                        self.warnings.push(Diagnostic::new(
                            **b_span,
                            format!("`{a}` and `{b}` are both accounts; is one of them a typo?"),
                        ));
                    }
                }
            }
        }
    }

    /// Orders the entries so that each fires after the entries that move
    /// what it reads, on days they both fire, and after the entries it names
    /// with `after`. Entries that don't depend on each other keep the order
    /// they're written in.
    fn order_entries(&mut self) {
        let index: HashMap<&str, usize> = self
            .entries
            .iter()
            .enumerate()
            .map(|(i, e)| (e.name.as_str(), i))
            .collect();
        // What each entry moves money into or out of, and the tags it moves
        // with, and where.
        let mut writes: Vec<Vec<(&Path, Span)>> = Vec::new();
        let mut tags: Vec<Vec<(&str, Span)>> = Vec::new();
        // What each entry reads, and where.
        let mut reads: Vec<Vec<(Read, Span)>> = Vec::new();
        for entry in &self.entries {
            let (mut w, mut t, mut r) = (Vec::new(), Vec::new(), Vec::new());
            let mut read = |e: &SpannedExpr| {
                walk_expr(e, &mut |sub| match sub.0.as_ref() {
                    Expr::Balance(path) | Expr::AccountTotal(path, _) => {
                        r.push((Read::Account(path.clone()), sub.1));
                    }
                    Expr::TagTotal { entry, tag, .. } => {
                        r.push((Read::Tag(index[entry.as_str()], tag.clone()), sub.1));
                    }
                    _ => {}
                });
            };
            for stmt in &entry.body {
                match stmt {
                    EntryStmt::Let { value, .. } => read(value),
                    EntryStmt::Move(m) => {
                        read(&m.amount);
                        w.push((&m.from.0, m.from.1));
                        w.push((&m.to.0, m.to.1));
                        if let Some((tag, tag_span)) = &m.tag {
                            t.push((tag.as_str(), *tag_span));
                        }
                    }
                }
            }
            writes.push(w);
            tags.push(t);
            reads.push(r);
        }

        // Why entry `i` fires before entry `j`: where `j` reads something,
        // what it is, and where `i` moves it.
        let mut before: Before = BTreeMap::new();
        for (j, entry_reads) in reads.iter().enumerate() {
            for (read, read_span) in entry_reads {
                let writers: Vec<(usize, Span, String)> = match read {
                    Read::Account(path) => (0..writes.len())
                        .filter_map(|i| {
                            let (_, span) = writes[i].iter().find(|(w, _)| w.starts_with(path))?;
                            Some((i, *span, format!("`{path}`")))
                        })
                        .collect(),
                    Read::Tag(i, tag) => tags[*i]
                        .iter()
                        .filter(|(t, _)| t == tag)
                        .take(1)
                        .map(|(_, span)| (*i, *span, format!("`{}.{tag}`", self.entries[*i].name)))
                        .collect(),
                };
                for (i, write_span, what) in writers {
                    if i != j {
                        before
                            .entry((i, j))
                            .or_insert(Some((*read_span, what, write_span)));
                    }
                }
            }
        }
        // `after` wins over what an entry reads.
        for (j, entry) in self.entries.iter().enumerate() {
            for (other, _) in &entry.after {
                let i = index[other.as_str()];
                before.remove(&(j, i));
                before.insert((i, j), None);
            }
        }

        let mut dependents: Vec<Vec<usize>> = vec![Vec::new(); self.entries.len()];
        for &(i, j) in before.keys() {
            dependents[i].push(j);
        }
        let (order, had_cycle) = crate::util::topological_sort(&dependents);
        if had_cycle {
            self.report_entry_cycle(&order, &before);
            return;
        }
        let mut entries: Vec<Option<EntryDef>> = std::mem::take(&mut self.entries)
            .into_iter()
            .map(Some)
            .collect();
        self.entries = order
            .into_iter()
            .map(|i| entries[i].take().expect("each entry is ordered once"))
            .collect();
    }

    /// Reports a cycle among the entries that `order` leaves out, which each
    /// read what the next one moves.
    fn report_entry_cycle(&mut self, order: &[usize], before: &Before) {
        let ordered: HashSet<usize> = order.iter().copied().collect();
        let next = |i: usize| {
            before
                .keys()
                .find(|&&(a, b)| a == i && !ordered.contains(&b))
                .map(|&(_, b)| b)
        };
        // Follow the edges from an unordered entry until one repeats.
        let start = (0..self.entries.len())
            .find(|i| !ordered.contains(i))
            .expect("a cycle leaves an entry out");
        let mut path = vec![start];
        let mut current = start;
        while let Some(n) = next(current) {
            if let Some(pos) = path.iter().position(|&p| p == n) {
                path.drain(..pos);
                break;
            }
            path.push(n);
            current = n;
        }
        let names: Vec<String> = path
            .iter()
            .map(|&i| format!("`{}`", self.entries[i].name))
            .collect();
        let message = if let [a, b] = names.as_slice() {
            format!(
                "{a} and {b} each read what the other moves, so it isn't clear which fires first"
            )
        } else {
            format!(
                "{} each read what the next moves, so it isn't clear which fires first",
                names.join(", ")
            )
        };
        // Each entry in the cycle reads something the one before it moves.
        let mut notes = Vec::new();
        for (k, &reader) in path.iter().enumerate() {
            let writer = path[(k + path.len() - 1) % path.len()];
            if let Some(Some((read_span, what, write_span))) = before.get(&(writer, reader)) {
                notes.push((
                    *read_span,
                    format!("`{}` reads {what} here", self.entries[reader].name),
                ));
                notes.push((
                    *write_span,
                    format!("which `{}` moves here", self.entries[writer].name),
                ));
            }
        }
        let (a, b) = (&self.entries[path[0]].name, &self.entries[path[1]].name);
        let primary = notes
            .first()
            .map_or(self.entries[path[0]].span, |(span, _)| *span);
        let mut diag = Diagnostic::new(primary, message).with_note(
            primary,
            format!("to choose, add `after {a}` to `{b}`, or the other way round"),
        );
        for (span, note) in notes {
            diag = diag.with_note(span, note);
        }
        self.diags.push(diag);
    }

    fn check_param_cycles(&mut self) {
        let known: HashSet<String> = self.params.keys().cloned().collect();
        let deps: HashMap<&str, Vec<String>> = self
            .params
            .iter()
            .map(|(name, body)| (name.as_str(), param_deps(body, &known)))
            .collect();
        let mut names: Vec<&str> = deps.keys().copied().collect();
        names.sort();
        let mut reported: HashSet<&str> = HashSet::new();
        for name in names {
            if reported.contains(name) {
                continue;
            }
            let Some(cycle) = find_cycle(name, &deps) else {
                continue;
            };
            reported.extend(cycle.iter().copied());
            let path: Vec<&str> = cycle.iter().copied().chain([name]).collect();
            self.diags.push(Diagnostic::new(
                self.param_spans[name],
                format!("`{name}` depends on itself ({})", path.join(" → ")),
            ));
        }
    }

    /// Reports values that read balances or totals. Values depend on nothing
    /// but the date, so they can be worked out for any day.
    fn check_values_read_no_state(&mut self) {
        let mut diags = Vec::new();
        let mut visit = |e: &SpannedExpr| {
            walk_expr(e, &mut |sub| {
                let what = match sub.0.as_ref() {
                    Expr::Balance(_) => "balances",
                    Expr::TagTotal { .. } | Expr::AccountTotal(..) => "totals",
                    _ => return,
                };
                diags.push(Diagnostic::new(
                    sub.1,
                    format!(
                        "values can't read {what}, which change as entries fire; \
                         read this in an entry's `let` or in a check"
                    ),
                ));
            });
        };
        for body in self.params.values() {
            match body {
                ParamBody::Const(e) => visit(e),
                ParamBody::Arms(arms) => {
                    for arm in arms {
                        visit(&arm.value);
                        if let Some(growth) = &arm.growth {
                            visit(&growth.by);
                        }
                    }
                }
            }
        }
        self.diags.extend(diags);
    }

    fn into_model(self) -> Model {
        Model {
            accounts: self
                .leaves
                .keys()
                .map(|path| {
                    let account = self
                        .accounts
                        .get(path)
                        .cloned()
                        .unwrap_or(Account { opening: None });
                    (path.clone(), account)
                })
                .collect(),
            params: topo_sort_by_deps(self.params, param_deps),
            fns: topo_sort_by_deps(self.fns, |def, known| fn_call_deps(&def.body, known)),
            entries: self.entries,
            checks: self.checks,
            warnings: self.warnings,
        }
    }
}

fn param_deps(body: &ParamBody, known: &HashSet<String>) -> Vec<String> {
    let mut deps: Vec<String> = Vec::new();
    let mut visitor = |e: &SpannedExpr| {
        if let Expr::Global(name) = e.0.as_ref()
            && known.contains(name)
        {
            deps.push(name.clone());
        }
    };
    match body {
        ParamBody::Const(e) => walk_expr(e, &mut visitor),
        ParamBody::Arms(arms) => {
            for arm in arms {
                walk_expr(&arm.value, &mut visitor);
                if let Some(growth) = &arm.growth {
                    walk_expr(&growth.by, &mut visitor);
                }
            }
        }
    }
    deps.sort();
    deps.dedup();
    deps
}

/// Finds the shortest dependency path from `start` back to itself, returned as
/// `[start, …]` without repeating `start` at the end.
fn find_cycle<'a>(start: &'a str, deps: &'a HashMap<&str, Vec<String>>) -> Option<Vec<&'a str>> {
    let mut parent: HashMap<&str, &str> = HashMap::new();
    let mut queue = VecDeque::from([start]);
    while let Some(node) = queue.pop_front() {
        for dep in &deps[node] {
            let dep = dep.as_str();
            if dep == start {
                let mut path = vec![node];
                while let Some(&prev) = path.last().and_then(|n| parent.get(n)) {
                    path.push(prev);
                }
                path.reverse();
                return Some(path);
            }
            if !parent.contains_key(dep) {
                parent.insert(dep, node);
                queue.push_back(dep);
            }
        }
    }
    None
}

/// Orders `items` so each comes after the items it depends on, as reported by
/// `deps(item, known_names)`. Ties are broken by name for deterministic output.
/// Members of cycles (reported elsewhere) are appended at the end.
fn topo_sort_by_deps<T>(
    mut items: HashMap<String, T>,
    deps: impl Fn(&T, &HashSet<String>) -> Vec<String>,
) -> IndexMap<String, T> {
    let known: HashSet<String> = items.keys().cloned().collect();
    let mut names: Vec<String> = items.keys().cloned().collect();
    names.sort();
    let idx: HashMap<&str, usize> = names
        .iter()
        .enumerate()
        .map(|(i, n)| (n.as_str(), i))
        .collect();

    let mut dependents: Vec<Vec<usize>> = vec![vec![]; names.len()];
    for (name, item) in &items {
        let i = idx[name.as_str()];
        for dep in deps(item, &known) {
            dependents[idx[dep.as_str()]].push(i);
        }
    }
    // Sort each adjacency list so newly-ready nodes are enqueued in name order.
    for d in &mut dependents {
        d.sort();
    }

    let (order, _) = crate::util::topological_sort(&dependents);
    let mut result = IndexMap::new();
    for i in order {
        let name = &names[i];
        if let Some(item) = items.remove(name) {
            result.insert(name.clone(), item);
        }
    }
    let mut remaining: Vec<String> = items.keys().cloned().collect();
    remaining.sort();
    for name in remaining {
        let item = items.remove(&name).expect("name taken from items");
        result.insert(name, item);
    }
    result
}

pub(crate) fn walk_expr(e: &SpannedExpr, f: &mut impl FnMut(&SpannedExpr)) {
    f(e);
    match e.0.as_ref() {
        Expr::Num(_)
        | Expr::Bool(_)
        | Expr::Ref(_)
        | Expr::Days(_)
        | Expr::Global(_)
        | Expr::Local(_)
        | Expr::Balance(_)
        | Expr::TagTotal { .. }
        | Expr::AccountTotal(..) => {}
        Expr::Neg(x) | Expr::Not(x) | Expr::Per(x, _) | Expr::Period(x, _) => walk_expr(x, f),
        Expr::Bin(a, _, b) => {
            walk_expr(a, f);
            walk_expr(b, f);
        }
        Expr::If { cond, then, else_ } => {
            walk_expr(cond, f);
            walk_expr(then, f);
            walk_expr(else_, f);
        }
        Expr::Call(_, args) => {
            for a in args {
                walk_expr(a, f);
            }
        }
        Expr::Range { from, to, .. } => {
            walk_expr(from, f);
            if let Some(to) = to {
                walk_expr(to, f);
            }
        }
    }
}

fn fn_call_deps(body: &[Stmt], known: &HashSet<String>) -> Vec<String> {
    let mut deps: Vec<String> = Vec::new();
    let mut visitor = |e: &SpannedExpr| {
        if let Expr::Call(Path(name), _) = e.0.as_ref()
            && let [name] = name.as_slice()
            && known.contains(name)
        {
            deps.push(name.clone());
        }
    };
    for stmt in body {
        match stmt {
            Stmt::Let { value, .. } => walk_expr(value, &mut visitor),
            Stmt::Return(expr) => walk_expr(expr, &mut visitor),
        }
    }
    deps.sort();
    deps.dedup();
    deps
}

/// Reports functions that call themselves, directly or through others.
fn validate_fn_calls(fns: &HashMap<String, FnDef>, diags: &mut Vec<Diagnostic>) {
    let names: HashSet<String> = fns.keys().cloned().collect();
    let mut sorted: Vec<&String> = fns.keys().collect();
    sorted.sort();
    for name in sorted {
        let def = &fns[name];
        let mut visited: HashSet<String> = HashSet::new();
        let mut stack: Vec<String> = fn_call_deps(&def.body, &names);
        while let Some(callee) = stack.pop() {
            if callee == *name {
                diags.push(Diagnostic::new(
                    def.span,
                    format!("function `{name}` calls itself"),
                ));
                break;
            }
            if visited.insert(callee.clone())
                && let Some(callee_def) = fns.get(&callee)
            {
                stack.extend(fn_call_deps(&callee_def.body, &names));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn resolve_src(src: &str) -> Result<Model, Vec<Diagnostic>> {
        let tokens = crate::lexer::lex(src, 0).expect("lex failed");
        resolve(&crate::parser::parse(tokens).expect("parse failed"))
    }

    fn errors(src: &str) -> Vec<String> {
        match resolve_src(src) {
            Ok(_) => panic!("`{src}` resolved without errors"),
            Err(diags) => diags.into_iter().map(|d| d.message).collect(),
        }
    }

    fn has_error(src: &str, needle: &str) {
        let errors = errors(src);
        assert!(
            errors.iter().any(|e| e.contains(needle)),
            "expected an error containing {needle:?}, got {errors:?}"
        );
    }

    #[test]
    fn accounts_come_from_moves_and_balances() {
        let model = resolve_src(
            "balances @ 2026-01-01 { assets.cash = 5 }
             entry pay @ last { income.salary -> assets.cash : 1 }",
        )
        .unwrap();
        let accounts: Vec<String> = model.accounts.keys().map(Path::join).collect();
        assert_eq!(accounts, ["assets.cash", "income.salary"]);
        let cash = Path(vec!["assets".into(), "cash".into()]);
        assert!(model.accounts[&cash].opening.is_some());
    }

    #[test]
    fn reading_an_account_nothing_moves_is_an_error() {
        has_error(
            "entry pay { income.salary -> assets.cash : 1 }
             check assets.csh >= 0",
            "no entry or balance moves money into or out of `assets.csh`",
        );
        // Groups of accounts and whole kinds can be read.
        assert!(
            resolve_src(
                "entry pay { income.salary -> assets.bank.cash : 1 }
                 check assets.bank >= 0 and assets >= 0 and liabilities >= 0"
            )
            .is_ok()
        );
    }

    #[test]
    fn an_account_cant_also_be_a_group() {
        has_error(
            "entry a { income.x -> assets.cash : 1 }
             entry b { income.x -> assets.cash.checking : 1 }",
            "`assets.cash` holds money, so it can't also be a group of accounts",
        );
    }

    #[test]
    fn moves_are_between_accounts() {
        has_error(
            "entry a { cash -> assets.cash : 1 }",
            "`cash` isn't an account",
        );
        has_error(
            "entry a { income -> assets.cash : 1 }",
            "holds every account of its kind",
        );
    }

    #[test]
    fn names_resolve_in_their_module_then_at_the_top() {
        let model = resolve_src(
            "let rate = 1
             mod loans {
               let rate = 2
               let doubled = rate * 2
               let top = top_level
             }
             let top_level = 3
             let from_outside = loans.rate",
        )
        .unwrap();
        let ParamBody::Const(e) = &model.params["loans.doubled"] else {
            panic!()
        };
        assert!(
            matches!(&*e.0, Expr::Bin(a, ..) if matches!(&*a.0, Expr::Global(n) if n == "loans.rate"))
        );
        let ParamBody::Const(e) = &model.params["from_outside"] else {
            panic!()
        };
        assert!(matches!(&*e.0, Expr::Global(n) if n == "loans.rate"));
    }

    #[test]
    fn unknown_names_suggest_close_ones() {
        let diags = resolve_src("let salary = 1\nlet x = salry").unwrap_err();
        assert_eq!(diags[0].message, "unknown name `salry`");
        assert!(
            diags[0]
                .extra
                .iter()
                .any(|(_, note)| note == "did you mean `salary`?")
        );
    }

    #[test]
    fn reserved_names_and_duplicates() {
        has_error("let may = 1", "`may` is a set of days");
        has_error("let assets = 1", "`assets` starts the names of accounts");
        has_error("let x = 1\nlet x = 2", "`x` is already declared");
        has_error("fn min(a, b) { a }", "`min` is a built-in function");
    }

    #[test]
    fn sets_of_days_and_values_dont_mix() {
        has_error(
            "let payday = 15th | last\nlet x = payday + 1",
            "is a set of days",
        );
        has_error(
            "let n = 2\nentry e @ n { income.a -> assets.b : 1 }",
            "is a number, not a set of days",
        );
        assert!(
            resolve_src("let payday = 15th | last\nentry e @ payday { income.a -> assets.b : 1 }")
                .is_ok()
        );
    }

    #[test]
    fn day_sets_can_be_named_but_not_circularly() {
        has_error(
            "let a = b | fri\nlet b = a & mon",
            "defined in terms of itself",
        );
    }

    #[test]
    fn tags_are_read_through_totals() {
        let model = resolve_src(
            "entry pay {
               income.salary -> assets.cash : 1
               assets.cash -> assets.retirement : 1 - contribution[year] #contribution
             }
             check pay.contribution[year] <= 1",
        )
        .unwrap();
        assert!(
            matches!(&*model.checks[0].1.0, Expr::Bin(a, ..) if matches!(&*a.0, Expr::TagTotal { tag, .. } if tag == "contribution"))
        );
        has_error(
            "check pay.contribution >= 0\nentry pay { income.a -> assets.b : 1 #contribution }",
            "is a tag",
        );
        has_error(
            "entry pay { income.a -> assets.b : 1 }\ncheck pay.nothing[year] >= 0",
            "has no tag `nothing`",
        );
        has_error("let x = 1\ncheck x[year] >= 0", "`x` has no total");
        has_error(
            "entry pay { income.a -> assets.b : 1 #t }\ncheck pay.t[week] >= 0",
            "not a week",
        );
    }

    #[test]
    fn functions_cant_read_accounts() {
        has_error(
            "entry e { income.a -> assets.b : 1 }\nfn f(x) { x + assets.b }",
            "can't read balances",
        );
    }

    #[test]
    fn arms_after_the_catch_all_never_apply() {
        has_error("let x { _ => 1\n 2026 => 2 }", "comes after `_`");
        has_error("let x { q2 => 1\n may => 2 }", "never applies");
        has_error(
            "let x { jul => 1 compounding 3% every year }",
            "needs an arm with a start",
        );
    }
}
