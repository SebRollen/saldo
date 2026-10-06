//! Resolves the names in a model: what each name in each module stands for,
//! which accounts exist, and which `let`s are values and which are sets of
//! days. Produces a [`Model`] in which every name is spelled out in full.

use crate::ast::schedule::{self, DaySet, Dow, Month, quarter_from_name};
use crate::ast::{
    AccountKind, AggKind, BinOp, Decl, Dist, EntryStmt, Expr, Growth, LetBody, ModBody, Move,
    Opening, Path, Program, Schedule, Span, Spanned, SpannedExpr, Stmt, TimeUnit,
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

/// What a total that a `balances` block sets counts.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Counted {
    /// A tag, by its entry's full name and its own.
    Tag(String, String),
    Account(Path),
}

/// A total that a `balances` block sets: what `counted` has moved so far in
/// the `agg` period that contains `date`, as of that day.
#[derive(Debug, Clone)]
pub struct TotalSoFar {
    pub counted: Counted,
    pub agg: AggKind,
    pub date: NaiveDate,
    pub value: SpannedExpr,
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
    /// The totals `balances` blocks set.
    pub totals_so_far: Vec<TotalSoFar>,
    /// Problems that don't stop the model from running.
    pub warnings: Vec<Diagnostic>,
}

pub fn resolve(program: &Program) -> Result<Model, Vec<Diagnostic>> {
    let program = crate::expand::expand(program)?;
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

/// Where the names in an expression resolve.
struct Scope<'s> {
    module: &'s [String],
    /// Function parameters and `let`s in scope.
    locals: &'s [String],
    /// The entry being resolved, and its tags.
    entry: Option<(&'s str, &'s HashSet<String>)>,
    /// The function being resolved, which can't read accounts or totals.
    function: Option<&'s str>,
    /// The variables of the `for` loops in scope, which stand for accounts.
    loops: &'s [String],
    /// Why random draws can't be written here, if they can't.
    no_draws: Option<&'static str>,
}

#[derive(Default)]
struct Resolver<'a> {
    diags: Vec<Diagnostic>,
    /// The functions that return modules, by full name.
    module_fns: HashSet<String>,
    /// The names declared in each module, by module path.
    namespaces: HashMap<Vec<String>, HashMap<String, (Symbol, Span)>>,
    lets: Vec<LetDecl<'a>>,
    classes: Vec<Option<Class>>,
    fn_decls: Vec<FnDecl<'a>>,
    entry_decls: Vec<EntryDecl<'a>>,
    check_decls: Vec<(Vec<String>, &'a SpannedExpr, Option<&'a SpannedExpr>)>,
    balance_decls: Vec<(Vec<String>, NaiveDate, &'a [Opening])>,

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
    totals_so_far: Vec<TotalSoFar>,
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

/// The first part of the path a `for` loop's variable stands for until the
/// loop is expanded, which no name can start with.
const LOOP: &str = "$";

/// The path that `for` loop variable `var` stands for until the loop is
/// expanded.
fn loop_account(var: &str) -> Path {
    Path(vec![LOOP.to_string(), var.to_string()])
}

/// Replaces `placeholder`, a loop's variable, with `account` in `stmts`.
fn substitute_stmts(stmts: &mut [EntryStmt], placeholder: &Path, account: &Path) {
    let fill = |path: &mut Path| {
        if path == placeholder {
            *path = account.clone();
        }
    };
    for stmt in stmts {
        match stmt {
            EntryStmt::Let { value, .. } => substitute_expr(value, placeholder, account),
            EntryStmt::Move(m) => {
                fill(&mut m.from.0);
                fill(&mut m.to.0);
                substitute_expr(&mut m.amount, placeholder, account);
            }
            EntryStmt::If {
                branches,
                otherwise,
            } => {
                for (cond, body) in branches {
                    substitute_expr(cond, placeholder, account);
                    substitute_stmts(body, placeholder, account);
                }
                substitute_stmts(otherwise, placeholder, account);
            }
            EntryStmt::For { body, .. } => substitute_stmts(body, placeholder, account),
        }
    }
}

fn substitute_expr((expr, _): &mut SpannedExpr, placeholder: &Path, account: &Path) {
    let sub = |e: &mut SpannedExpr| substitute_expr(e, placeholder, account);
    match expr.as_mut() {
        Expr::Balance(path) | Expr::AccountTotal(path, _) => {
            if path == placeholder {
                *path = account.clone();
            }
        }
        Expr::Neg(x) | Expr::Not(x) | Expr::Per(x, _) | Expr::Period(x, _) | Expr::Named(_, x) => {
            sub(x)
        }
        Expr::Bin(a, _, b) => {
            sub(a);
            sub(b);
        }
        Expr::If { cond, then, else_ } => {
            sub(cond);
            sub(then);
            sub(else_);
        }
        Expr::Call(_, args) => args.iter_mut().for_each(sub),
        // A draw in a loop is drawn separately for each account.
        Expr::Draw(_, args, site) => {
            *site = crate::random::mix(*site, crate::random::hash(account.join().as_bytes()));
            args.iter_mut().for_each(sub);
        }
        Expr::Range { from, to, .. } => {
            sub(from);
            if let Some(to) = to {
                sub(to);
            }
        }
        Expr::Num(_)
        | Expr::Bool(_)
        | Expr::Ref(_)
        | Expr::Days(_)
        | Expr::Global(_)
        | Expr::Local(_)
        | Expr::TagTotal { .. } => {}
    }
}

/// Gives each draw in `exprs`, the expressions of declaration `full` in the
/// order they're written, its site: the declaration's name and how many draws
/// come before it. Loops then mix in the account each copy is for.
fn number_draws<'e>(full: &str, exprs: impl IntoIterator<Item = &'e mut SpannedExpr>) {
    fn number(e: &mut SpannedExpr, decl: u64, n: &mut u64) {
        if let Expr::Draw(_, _, site) = e.0.as_mut() {
            *site = crate::random::mix(decl, *n);
            *n += 1;
        }
        crate::expand::for_each_child(e.0.as_mut(), &mut |x| number(x, decl, n));
    }
    let decl = crate::random::hash(full.as_bytes());
    let mut n = 0;
    for e in exprs {
        number(e, decl, &mut n);
    }
}

/// The expressions in `stmts`, in the order they're written.
fn entry_exprs<'s>(stmts: &'s mut [EntryStmt], out: &mut Vec<&'s mut SpannedExpr>) {
    for stmt in stmts {
        match stmt {
            EntryStmt::Let { value, .. } => out.push(value),
            EntryStmt::Move(m) => out.push(&mut m.amount),
            EntryStmt::If {
                branches,
                otherwise,
            } => {
                for (cond, body) in branches {
                    out.push(cond);
                    entry_exprs(body, out);
                }
                entry_exprs(otherwise, out);
            }
            EntryStmt::For { body, .. } => entry_exprs(body, out),
        }
    }
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
                    if let Some(Stmt::Mod(_)) = body.last() {
                        // Expanded where it's used; it can't be called for a value.
                        if self.declare(module, name, Symbol::Fn(full.clone())) {
                            self.module_fns.insert(full);
                        }
                        continue;
                    }
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
                    let mut tags: HashSet<String> = HashSet::new();
                    EntryStmt::visit(body, &mut |stmt| {
                        if let EntryStmt::Move(Move { tag: Some(tag), .. }) = stmt {
                            tags.insert(tag.0.clone());
                        }
                    });
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
        // A path that starts at the top of the model, which only functions'
        // arguments do, looks up nothing in `module`.
        let (module, path) = match path.0.split_first() {
            Some((root, rest)) if root == Path::ROOT => (&[][..], &Path(rest.to_vec())),
            _ => (module, path),
        };
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
                loops: &[],
                no_draws: Some("a check can't draw at random; draw in a value, and check that"),
            };
            let expr = self.resolve_expr(expr, &scope);
            if let Some(schedule) = self.schedule(when, &module) {
                self.checks.push((schedule, expr));
            }
        }
        for i in 0..self.balance_decls.len() {
            let (module, date, balances) = self.balance_decls[i].clone();
            for opening in balances {
                let (path, span) = &opening.target;
                if let Some(unit) = opening.period {
                    self.resolve_total_so_far(&module, date, opening, unit);
                    continue;
                }
                if !self.account_path(path, *span) {
                    continue;
                }
                let scope = Scope {
                    module: &module,
                    locals: &[],
                    entry: None,
                    function: None,
                    loops: &[],
                    no_draws: Some("an opening balance can't be drawn at random"),
                };
                let value = self.resolve_expr(&opening.value, &scope);
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
        self.expand_loops();
        if self.diags.is_empty() {
            self.order_entries();
        }
    }

    /// `x[unit] = value` in a `balances` block in `module` on `date`: what a
    /// tag or an account has moved so far that period.
    fn resolve_total_so_far(
        &mut self,
        module: &[String],
        date: NaiveDate,
        opening: &Opening,
        unit: TimeUnit,
    ) {
        let (path, span) = &opening.target;
        let Some(agg) = AggKind::from_unit(unit) else {
            self.diags.push(Diagnostic::new(
                *span,
                format!("totals are kept for a year, quarter or month, not a {unit}"),
            ));
            return;
        };
        let counted = match self.lookup(module, path, *span) {
            Ok(Target::Tag(entry, tag)) => Counted::Tag(entry, tag),
            Ok(Target::Account(path)) => {
                if !self.account_path(&path, *span) {
                    return;
                }
                self.leaves.entry(path.clone()).or_insert(*span);
                Counted::Account(path)
            }
            Ok(_) => {
                self.diags.push(Diagnostic::new(
                    *span,
                    format!(
                        "`{path}` has no total; only tags and accounts do, like \
                         `paycheck.contribution[year]`"
                    ),
                ));
                return;
            }
            Err(diag) => {
                self.diags.push(diag);
                return;
            }
        };
        if self
            .totals_so_far
            .iter()
            .any(|s| s.counted == counted && s.agg == agg && s.date == date)
        {
            self.diags.push(Diagnostic::new(
                *span,
                format!("`{path}[{unit}]` is already set on {date}"),
            ));
            return;
        }
        let scope = Scope {
            module,
            locals: &[],
            entry: None,
            function: None,
            loops: &[],
            no_draws: Some("a total so far can't be drawn at random"),
        };
        let value = self.resolve_expr(&opening.value, &scope);
        self.totals_so_far.push(TotalSoFar {
            counted,
            agg,
            date,
            value,
        });
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
            loops: &[],
            no_draws: None,
        };
        let mut body = match self.lets[i].body {
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
        match &mut body {
            ParamBody::Const(e) => number_draws(&full, [e]),
            ParamBody::Arms(arms) => number_draws(
                &full,
                arms.iter_mut().flat_map(|arm| {
                    std::iter::once(&mut arm.value).chain(arm.growth.as_mut().map(|g| &mut g.by))
                }),
            ),
        }
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
                loops: &[],
                no_draws: None,
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
                Stmt::Mod(_) => unreachable!("functions that make modules aren't resolved"),
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
        let mut resolved =
            self.resolve_entry_stmts(body, &module, &full, &tags, &mut Vec::new(), &[]);
        let mut exprs = Vec::new();
        entry_exprs(&mut resolved, &mut exprs);
        number_draws(&full, exprs);
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

    /// Resolves the statements of entry `full`, in which `locals` are the
    /// `let`s in scope and `loops` the variables of the `for` loops around
    /// them; `let`s in a branch or a loop are only in scope in it.
    fn resolve_entry_stmts(
        &mut self,
        stmts: &[EntryStmt],
        module: &[String],
        full: &str,
        tags: &HashSet<String>,
        locals: &mut Vec<String>,
        loops: &[String],
    ) -> Vec<EntryStmt> {
        let mut resolved = Vec::new();
        for stmt in stmts {
            let scope = Scope {
                module,
                locals,
                entry: Some((full, tags)),
                function: None,
                loops,
                no_draws: None,
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
                    let mut endpoint = |(path, path_span): &Spanned<Path>| {
                        if let [var] = path.0.as_slice()
                            && loops.contains(var)
                        {
                            return (loop_account(var), *path_span);
                        }
                        if self.account_path(path, *path_span) {
                            self.leaves.entry(path.clone()).or_insert(*path_span);
                        }
                        (path.clone(), *path_span)
                    };
                    let (from, to) = (endpoint(&m.from), endpoint(&m.to));
                    resolved.push(EntryStmt::Move(Move {
                        from,
                        to,
                        amount,
                        ..m.clone()
                    }));
                }
                EntryStmt::If {
                    branches,
                    otherwise,
                } => {
                    let mut resolved_branches = Vec::new();
                    for (cond, body) in branches {
                        let cond = self.resolve_expr(cond, &scope);
                        let body = self.resolve_entry_stmts(
                            body,
                            module,
                            full,
                            tags,
                            &mut locals.clone(),
                            loops,
                        );
                        resolved_branches.push((cond, body));
                    }
                    let otherwise = self.resolve_entry_stmts(
                        otherwise,
                        module,
                        full,
                        tags,
                        &mut locals.clone(),
                        loops,
                    );
                    resolved.push(EntryStmt::If {
                        branches: resolved_branches,
                        otherwise,
                    });
                }
                EntryStmt::For { var, over, body } => {
                    for (path, path_span) in over {
                        self.account_path(path, *path_span);
                    }
                    let mut loops = loops.to_vec();
                    loops.push(var.0.clone());
                    let body = self.resolve_entry_stmts(
                        body,
                        module,
                        full,
                        tags,
                        &mut locals.clone(),
                        &loops,
                    );
                    resolved.push(EntryStmt::For {
                        var: var.clone(),
                        over: over.clone(),
                        body,
                    });
                }
            }
        }
        resolved
    }

    /// Replaces every `for` loop with its statements once for each account it
    /// goes through, now that every account is known.
    fn expand_loops(&mut self) {
        let mut entries = std::mem::take(&mut self.entries);
        for entry in &mut entries {
            let body = std::mem::take(&mut entry.body);
            entry.body = self.expand_stmts(body);
        }
        self.entries = entries;
    }

    fn expand_stmts(&mut self, stmts: Vec<EntryStmt>) -> Vec<EntryStmt> {
        let mut out = Vec::new();
        for stmt in stmts {
            match stmt {
                EntryStmt::For { var, over, body } => {
                    let mut accounts: Vec<Path> = Vec::new();
                    for (path, path_span) in &over {
                        let under: Vec<&Path> =
                            self.leaves.keys().filter(|l| l.starts_with(path)).collect();
                        if under.is_empty() {
                            self.diags.push(Diagnostic::new(
                                *path_span,
                                format!("no entry or balance moves money into or out of `{path}`"),
                            ));
                        }
                        for leaf in under {
                            if !accounts.contains(leaf) {
                                accounts.push(leaf.clone());
                            }
                        }
                    }
                    // Inner loops first, so the accounts they go through are
                    // then filled in for this loop's variable too.
                    let body = self.expand_stmts(body);
                    let placeholder = loop_account(&var.0);
                    for account in accounts {
                        let mut body = body.clone();
                        substitute_stmts(&mut body, &placeholder, &account);
                        out.extend(body);
                    }
                }
                EntryStmt::If {
                    branches,
                    otherwise,
                } => out.push(EntryStmt::If {
                    branches: branches
                        .into_iter()
                        .map(|(cond, body)| (cond, self.expand_stmts(body)))
                        .collect(),
                    otherwise: self.expand_stmts(otherwise),
                }),
                stmt => out.push(stmt),
            }
        }
        out
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
            Expr::Call(callee, args) => match self.resolve_call(callee, args, span, scope) {
                Some(call) => call,
                None => return e.clone(),
            },
            Expr::Named((name, _), _) => {
                self.diags.push(Diagnostic::new(
                    span,
                    format!("only a call's arguments are given by name, like `f({name}: …)`"),
                ));
                return e.clone();
            }
            Expr::Per(x, unit) => Expr::Per(self.resolve_expr(x, scope), *unit),
            Expr::Period(x, unit) => match self.resolve_period(x, *unit, span, scope) {
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
            | Expr::AccountTotal(..)
            | Expr::Draw(..) => return e.clone(),
        };
        (Box::new(resolved), span)
    }

    fn resolve_ref(&mut self, path: &Path, span: Span, scope: &Scope) -> Option<Expr> {
        if let [var] = path.0.as_slice()
            && scope.loops.contains(var)
            && !scope.locals.contains(var)
        {
            return Some(Expr::Balance(loop_account(var)));
        }
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

    /// A call to `callee`, with its arguments in order.
    fn resolve_call(
        &mut self,
        callee: &Path,
        args: &[SpannedExpr],
        span: Span,
        scope: &Scope,
    ) -> Option<Expr> {
        let named = args.iter().any(|a| matches!(a.0.as_ref(), Expr::Named(..)));
        let (name, args) = if let [name] = callee.0.as_slice()
            && let Some((_, arity)) = BUILTINS.iter().find(|(builtin, _)| builtin == name)
        {
            if named {
                self.diags.push(Diagnostic::new(
                    span,
                    format!("`{name}` takes its arguments by position"),
                ));
                return None;
            }
            if args.len() != *arity {
                let word = if *arity == 1 { "argument" } else { "arguments" };
                self.diags.push(Diagnostic::new(
                    span,
                    format!("`{callee}` takes {arity} {word}, got {}", args.len()),
                ));
                return None;
            }
            (name.clone(), args.to_vec())
        } else {
            match self.lookup(scope.module, callee, span) {
                Ok(Target::Fn(full)) if self.module_fns.contains(&full) => {
                    self.diags.push(Diagnostic::new(
                        span,
                        format!("`{callee}` makes a module; use it like `mod x = {callee}(…)`"),
                    ));
                    return None;
                }
                Ok(Target::Fn(full)) => {
                    let decl = self
                        .fn_decls
                        .iter()
                        .find(|d| d.full == full)
                        .expect("declared functions are collected");
                    let args = crate::expand::order_args(
                        &callee.join(),
                        decl.params,
                        args,
                        span,
                        &mut self.diags,
                    )?;
                    (full, args)
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
        let args = args.iter().map(|a| self.resolve_expr(a, scope)).collect();
        let Some(dist) = Dist::from_name(&name) else {
            return Some(Expr::Call(Path(vec![name]), args));
        };
        let why = match scope.function {
            Some(function) => Some(format!(
                "function `{function}` can't draw at random, since every call would share \
                 its draws; pass the draw in as an argument"
            )),
            None => scope.no_draws.map(str::to_string),
        };
        if let Some(why) = why {
            self.diags.push(Diagnostic::new(span, why));
            return None;
        }
        // The declaration it's in numbers it (see `number_draws`).
        Some(Expr::Draw(dist, args, 0))
    }

    /// `x[unit]`: what a tag or an account has moved so far this period, or,
    /// for anything else, a limit: an amount each period.
    fn resolve_period(
        &mut self,
        x: &SpannedExpr,
        unit: TimeUnit,
        span: Span,
        scope: &Scope,
    ) -> Option<Expr> {
        let total = match x.0.as_ref() {
            Expr::Ref(path) => match (path.0.as_slice(), scope.entry) {
                ([var], _) if scope.loops.contains(var) && !scope.locals.contains(var) => {
                    Some(Target::Account(loop_account(var)))
                }
                ([tag], Some((entry, tags))) if tags.contains(tag) => {
                    Some(Target::Tag(entry.to_string(), tag.clone()))
                }
                ([local], _) if scope.locals.contains(local) => None,
                _ => match self.lookup(scope.module, path, x.1) {
                    Ok(target @ (Target::Tag(..) | Target::Account(_))) => Some(target),
                    Ok(_) => None,
                    Err(diag) => {
                        self.diags.push(diag);
                        return None;
                    }
                },
            },
            _ => None,
        };
        let Some(total) = total else {
            return Some(Expr::Period(self.resolve_expr(x, scope), unit));
        };
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
        Some(match total {
            Target::Tag(entry, tag) => Expr::TagTotal { entry, tag, agg },
            Target::Account(path) => {
                if path.0[0] != LOOP {
                    self.reads.push((path.clone(), x.1));
                }
                Expr::AccountTotal(path, agg)
            }
            _ => unreachable!("only tags and accounts have totals"),
        })
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
                    .with_note(**child_span, format!("`{child}` is here"))
                    .with_note(
                        **span,
                        format!(
                            "to move money into each account under `{leaf}`, \
                             use `for a in {leaf} {{ … }}`"
                        ),
                    ),
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
            EntryStmt::visit(&entry.body, &mut |stmt| match stmt {
                EntryStmt::Let { value, .. } => read(value),
                EntryStmt::Move(m) => {
                    read(&m.amount);
                    w.push((&m.from.0, m.from.1));
                    w.push((&m.to.0, m.to.1));
                    if let Some((tag, tag_span)) = &m.tag {
                        t.push((tag.as_str(), *tag_span));
                    }
                }
                EntryStmt::If { branches, .. } => {
                    for (cond, _) in branches {
                        read(cond);
                    }
                }
                EntryStmt::For { .. } => unreachable!("loops are expanded first"),
            });
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
            totals_so_far: self.totals_so_far,
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
        Expr::Neg(x) | Expr::Not(x) | Expr::Per(x, _) | Expr::Period(x, _) | Expr::Named(_, x) => {
            walk_expr(x, f)
        }
        Expr::Bin(a, _, b) => {
            walk_expr(a, f);
            walk_expr(b, f);
        }
        Expr::If { cond, then, else_ } => {
            walk_expr(cond, f);
            walk_expr(then, f);
            walk_expr(else_, f);
        }
        Expr::Call(_, args) | Expr::Draw(_, args, _) => {
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
            Stmt::Mod(_) => {}
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
        // Anything else is a limit.
        let model = resolve_src("let x = 1\ncheck x[year] >= 0").unwrap();
        assert!(
            matches!(&*model.checks[0].1.0, Expr::Bin(a, ..) if matches!(&*a.0, Expr::Period(..)))
        );
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
