use crate::ast::{
    Decl, Expr, ParamBody, Path, Posting, PostingAmount, Program, ScheduleRef, Schedule, Span, SpannedExpr, Stmt,
};
use crate::errors::Diagnostic;
use crate::eval::BUILTINS;
use chrono::NaiveDate;
use indexmap::IndexMap;
use std::collections::{HashMap, HashSet};
use crate::ast::schedule::{Periodic, Period};

#[derive(Debug, Clone)]
pub struct Account {
    pub opening: Option<(SpannedExpr, NaiveDate)>,
}

#[derive(Debug)]
pub struct EntryDef {
    pub label: String,
    /// Stable key for leg namespacing: equals the alias when present, otherwise a
    /// synthetic "$N" index. Never derived from the label, so duplicate labels are fine.
    pub key: String,
    pub schedule: Schedule,
    pub postings: Vec<Posting>,
    pub span: Span,
}


#[derive(Debug, Clone)]
pub struct FnDef {
    pub params: Vec<String>,
    pub body: Vec<Stmt>,
    pub span: Span,
}

#[derive(Debug)]
pub struct Model {
    /// Accounts in declaration order (used as column order in CSV output).
    pub stocks: IndexMap<Path, Account>,
    pub params: IndexMap<String, ParamBody>,
    pub fns: IndexMap<String, FnDef>,
    pub entries: Vec<EntryDef>,
    pub asserts: Vec<(Schedule, SpannedExpr)>,
    pub leg_names: HashSet<(String, String)>,
}

struct Resolver<'a> {
    program: &'a Program,
    diags: Vec<Diagnostic>,

    schedules: IndexMap<String, Schedule>,
    stocks: IndexMap<Path, Account>,
    params: HashMap<String, ParamBody>,
    fns: HashMap<String, FnDef>,
    entries: Vec<EntryDef>,
    asserts: Vec<(Schedule, SpannedExpr)>,
    leg_names: HashSet<(String, String)>,

    schedule_spans: HashMap<String, Span>,
    stock_spans: HashMap<Path, Span>,
    param_spans: HashMap<String, Span>,
    fn_spans: HashMap<String, Span>,
    entry_aliases: HashMap<String, Span>,
}

impl<'a> Resolver<'a> {
    fn new(program: &'a Program) -> Self {
        Resolver {
            program,
            diags: Vec::new(),
            schedules: IndexMap::new(),
            stocks: IndexMap::new(),
            params: HashMap::new(),
            fns: HashMap::new(),
            entries: Vec::new(),
            asserts: Vec::new(),
            leg_names: HashSet::new(),
            schedule_spans: HashMap::new(),
            stock_spans: HashMap::new(),
            param_spans: HashMap::new(),
            fn_spans: HashMap::new(),
            entry_aliases: HashMap::new(),
        }
    }

    fn resolve(mut self) -> Result<Model, Vec<Diagnostic>> {
        self.collect_schedules();
        self.collect_declarations();
        validate_fn_bodies(&self.fns, &mut self.diags);
        self.validate_references();
        if self.diags.is_empty() {
            Ok(self.into_model())
        } else {
            Err(self.diags)
        }
    }

    fn collect_schedules(&mut self) {
        let program = self.program;
        for (decl, span) in &program.decls {
            if let Decl::Schedule { name, schedule } = decl {
                if let Some(prev) = self.schedule_spans.get(name) {
                    self.diags.push(
                        Diagnostic::new(*span, format!("duplicate schedule `{name}`"))
                            .with_note(*prev, "previously declared here"),
                    );
                } else {
                    check_periodic_schedule(schedule, *span, &mut self.diags);
                    self.schedule_spans.insert(name.clone(), *span);
                    self.schedules.insert(name.clone(), schedule.clone());
                }
            }
        }
    }

    fn collect_declarations(&mut self) {
        let program = self.program;
        for (decl, span) in &program.decls {
            match decl {
                Decl::Account { name, opening } => {
                    if let Some(prev) = self.stock_spans.get(name) {
                        self.diags.push(
                            Diagnostic::new(*span, format!("duplicate account `{name}`"))
                                .with_note(*prev, "previously declared here"),
                        );
                    } else {
                        self.stock_spans.insert(name.clone(), *span);
                        self.stocks.insert(name.clone(), Account { opening: opening.clone() });
                    }
                }
                Decl::Schedule { .. } => {
                    // already processed in collect_schedules
                }
                Decl::Param { name, body, .. } => {
                    if let Some(prev) = self.param_spans.get(name) {
                        self.diags.push(
                            Diagnostic::new(*span, format!("duplicate param `{name}`"))
                                .with_note(*prev, "previously declared here"),
                        );
                        continue;
                    }
                    match body {
                        ParamBody::Const(e) => {
                            self.params.insert(name.clone(), ParamBody::Const(e.clone()));
                        }
                        ParamBody::Schedule(intervals) => {
                            let mut sorted = intervals.clone();
                            sorted.sort_by_key(|i| i.from);
                            for win in sorted.windows(2) {
                                let a = &win[0];
                                let b = &win[1];
                                let a_end = a.to.unwrap_or(chrono::NaiveDate::MAX);
                                if a_end > b.from {
                                    self.diags.push(Diagnostic::new(
                                        *span,
                                        format!("param `{name}` has overlapping intervals"),
                                    ));
                                    break;
                                }
                            }
                            for i in &sorted {
                                if let Some(to) = i.to
                                    && to <= i.from
                                {
                                    self.diags.push(Diagnostic::new(
                                        *span,
                                        format!("param `{name}` interval ends before it starts"),
                                    ));
                                }
                            }
                            self.params.insert(name.clone(), ParamBody::Schedule(sorted));
                        }
                    }
                    self.param_spans.insert(name.clone(), *span);
                }
                Decl::Entry {
                    label,
                    alias,
                    schedule,
                    postings,
                } => {
                    if let Some(a) = alias {
                        if let Some(prev) = self.entry_aliases.get(a) {
                            self.diags.push(
                                Diagnostic::new(*span, format!("duplicate entry alias `{a}`"))
                                    .with_note(*prev, "previously declared here"),
                            );
                            continue;
                        }
                    }
                    if postings.is_empty() {
                        self.diags.push(Diagnostic::new(
                            *span,
                            format!("entry `{label}` has no postings"),
                        ));
                    }
                    let auto_count = postings.iter().filter(|p| p.amount.is_none()).count();
                    if auto_count > 1 {
                        self.diags.push(Diagnostic::new(
                            *span,
                            format!("entry `{label}` has more than one auto-balance posting"),
                        ));
                    }

                    let key = alias.clone().unwrap_or_else(|| format!("${}", self.entries.len()));

                    let mut entry_leg_names: HashSet<String> = HashSet::new();
                    for posting in postings {
                        let Some(leg) = &posting.leg_name else { continue };
                        if !entry_leg_names.insert(leg.clone()) {
                            self.diags.push(Diagnostic::new(
                                *span,
                                format!("duplicate leg name `{leg}` in entry `{label}`"),
                            ));
                        } else if self.param_spans.contains_key(leg) {
                            self.diags.push(Diagnostic::new(
                                *span,
                                format!("leg name `{leg}` conflicts with a param of the same name"),
                            ));
                        } else {
                            self.leg_names.insert((key.clone(), leg.clone()));
                        }
                    }

                    if let Some(a) = alias {
                        self.entry_aliases.insert(a.clone(), *span);
                    }
                    // Clone early to release the borrow on self.schedules before
                    // accessing self.diags below.
                    let schedule: Schedule = match schedule {
                        ScheduleRef::Literal(s) => s.clone(),
                        ScheduleRef::Named(n) => {
                            match self.schedules.get(n).cloned() {
                                Some(s) => s,
                                None => {
                                    self.diags.push(Diagnostic::new(
                                        *span,
                                        format!("schedule `{n}` is not defined"),
                                    ));
                                    continue;
                                }
                            }
                        }
                    };
                    check_periodic_schedule(&schedule, *span, &mut self.diags);
                    let sorted_postings = topo_sort_postings(
                        postings.clone(),
                        &entry_leg_names,
                        *span,
                        label,
                        &mut self.diags,
                    );
                    self.entries.push(EntryDef {
                        label: label.clone(),
                        key,
                        schedule,
                        postings: sorted_postings,
                        span: *span,
                    });
                }
                Decl::Fn { name, params, body } => {
                    if let Some(prev) = self.fn_spans.get(name) {
                        self.diags.push(
                            Diagnostic::new(*span, format!("duplicate function `{name}`"))
                                .with_note(*prev, "previously declared here"),
                        );
                    } else {
                        let mut seen_params: HashSet<&str> = HashSet::new();
                        for p in params {
                            if !seen_params.insert(p.as_str()) {
                                self.diags.push(Diagnostic::new(
                                    *span,
                                    format!("duplicate parameter `{p}` in function `{name}`"),
                                ));
                            }
                        }
                        self.fn_spans.insert(name.clone(), *span);
                        self.fns.insert(name.clone(), FnDef {
                            params: params.clone(),
                            body: body.clone(),
                            span: *span,
                        });
                    }
                }
                Decl::Assert { schedule, asserted } => {
                    let schedule: Schedule = match schedule {
                        None => Schedule::Periodic(Periodic { period: Period::Day, nth: None, start: None }),
                        Some(ScheduleRef::Literal(s)) => s.clone(),
                        Some(ScheduleRef::Named(n)) => {
                            match self.schedules.get(n).cloned() {
                                Some(s) => s,
                                None => {
                                    self.diags.push(Diagnostic::new(
                                        *span,
                                        format!("schedule `{n}` is not defined"),
                                    ));
                                    continue;
                                }
                            }
                        }
                    };
                    check_periodic_schedule(&schedule, *span, &mut self.diags);
                    self.asserts.push((schedule, asserted.clone()));
                }
            }
        }
    }

    fn validate_references(&mut self) {
        let stock_set: HashSet<Path> = self.stocks.keys().cloned().collect();
        let param_set: HashSet<String> = self.params.keys().cloned().collect();
        let no_extra: HashSet<String> = HashSet::new();

        // `self.program` is &'a Program — copying the reference lets the loop iterate
        // program.decls without borrowing `self`, so we can call &mut self methods inside.
        let program = self.program;
        for (decl, _span) in &program.decls {
            match decl {
                Decl::Account { opening: Some((e, _)), .. } => {
                    self.check_expr(e, None, &no_extra, &stock_set, &param_set);
                }
                Decl::Param { body, .. } => match body {
                    ParamBody::Const(e) => {
                        self.check_expr(e, None, &no_extra, &stock_set, &param_set);
                    }
                    ParamBody::Schedule(intervals) => {
                        for i in intervals {
                            self.check_expr(&i.value, None, &no_extra, &stock_set, &param_set);
                        }
                    }
                },
                Decl::Assert { asserted, .. } => {
                    self.check_expr(asserted, None, &no_extra, &stock_set, &param_set);
                }
                _ => {}
            }
        }

        // Take entries out so we can call &mut self methods while iterating.
        let entries = std::mem::take(&mut self.entries);
        for entry in &entries {
            let entry_legs: HashSet<String> = entry.postings
                .iter()
                .filter_map(|p| p.leg_name.as_ref())
                .cloned()
                .collect();
            for posting in &entry.postings {
                self.check_path_is_stock(&posting.account, entry.span, &stock_set);
                if let Some(PostingAmount::Expr(e)) = &posting.amount {
                    self.check_expr(e, Some(&entry.key), &entry_legs, &stock_set, &param_set);
                }
            }
        }
        self.entries = entries;
    }

    fn check_expr(
        &mut self,
        e: &SpannedExpr,
        entry_context: Option<&str>,
        extra_refs: &HashSet<String>,
        stock_set: &HashSet<Path>,
        param_set: &HashSet<String>,
    ) {
        let mut local_diags: Vec<Diagnostic> = Vec::new();
        let leg_names = &self.leg_names;
        let fns = &self.fns;
        walk_expr(e, &mut |sub: &SpannedExpr| {
            if let Expr::Ref(path) = sub.0.as_ref()
                && resolve_ref(path, stock_set, param_set).is_none()
                && !(path.0.len() == 1 && extra_refs.contains(&path.0[0]))
            {
                local_diags.push(Diagnostic::new(
                    sub.1,
                    format!("unknown reference `{path}`"),
                ));
            }
            if let Expr::Call(name, args) = sub.0.as_ref() {
                if let Some((_, arity)) = BUILTINS.iter().find(|(n, _)| *n == name.as_str()) {
                    if args.len() != *arity {
                        let argument_str = if *arity == 1 { "argument" } else { "arguments" };
                        local_diags.push(Diagnostic::new(
                            sub.1,
                            format!("`{name}` takes {arity} {argument_str}, got {}", args.len()),
                        ));
                    }
                } else if let Some(def) = fns.get(name.as_str()) {
                    if args.len() != def.params.len() {
                        let expected = def.params.len();
                        local_diags.push(Diagnostic::new(
                            sub.1,
                            format!("`{name}` takes {expected} argument(s), got {}", args.len()),
                        ));
                    }
                } else {
                    local_diags.push(Diagnostic::new(
                        sub.1,
                        format!("unknown function `{name}`"),
                    ));
                }
            }
            if let Expr::ParamAgg(entry_opt, leg, _) = sub.0.as_ref() {
                let key: (String, String) = match entry_opt {
                    Some(entry) => (entry.clone(), leg.clone()),
                    None => match entry_context {
                        Some(e) => (e.to_string(), leg.clone()),
                        None => {
                            local_diags.push(Diagnostic::new(
                                sub.1,
                                format!("unqualified leg `{leg}` — use `<entry>.{leg}.ytd` outside of an entry"),
                            ));
                            return;
                        }
                    },
                };
                if !leg_names.contains(&key) {
                    local_diags.push(Diagnostic::new(
                        sub.1,
                        format!("unknown named leg `{}.{}`", key.0, key.1),
                    ));
                }
            }
        });
        self.diags.extend(local_diags);
    }

    fn check_path_is_stock(&mut self, p: &Path, span: Span, stock_set: &HashSet<Path>) {
        if !stock_set.contains(p) {
            self.diags.push(Diagnostic::new(span, format!("unknown account `{p}`")));
        }
    }

    fn into_model(self) -> Model {
        Model {
            stocks: self.stocks,
            params: topo_sort_params(self.params),
            fns: topo_sort_fns(self.fns),
            entries: self.entries,
            asserts: self.asserts,
            leg_names: self.leg_names,
        }
    }
}

pub fn resolve(program: &Program) -> Result<Model, Vec<Diagnostic>> {
    Resolver::new(program).resolve()
}

fn check_periodic_schedule(schedule: &Schedule, span: Span, diags: &mut Vec<Diagnostic>) {
    if let Schedule::Periodic(Periodic { nth: Some(_), start: None, period }) = schedule {
        let label = match period {
            Period::Day => "days",
            Period::Week { .. } => "weeks",
            Period::Weekday(_) => "weekdays",
            Period::NamedMonth { .. } => "months",
            Period::Month { .. } => "months",
            Period::Quarter => "quarters",
            Period::Year { .. } => "years",
        };
        diags.push(Diagnostic::new(
            span,
            format!("schedule with `every N {label}` requires a `from` date"),
        ));
    }
}

fn collect_param_deps(body: &ParamBody, known: &HashSet<String>) -> Vec<String> {
    let mut deps: Vec<String> = Vec::new();
    let mut visitor = |e: &SpannedExpr| {
        if let Expr::Ref(path) = e.0.as_ref() && path.0.len() == 1 && known.contains(&path.0[0]) {
            deps.push(path.0[0].clone());
        }
        if let Expr::ParamAgg(_, name, _) = e.0.as_ref() && known.contains(name) {
            deps.push(name.clone());
        }
    };
    match &body {
        ParamBody::Const(e) => walk_expr(e, &mut visitor),
        ParamBody::Schedule(intervals) => {
            for iv in intervals {
                walk_expr(&iv.value, &mut visitor);
            }
        }
    }
    deps.sort();
    deps.dedup();
    deps
}

/// Kahn's topological sort on `n` nodes numbered `0..n`.
/// Returns `(order, had_cycle)`. When `had_cycle` is true, `order.len() < n` and the
/// unreachable nodes are omitted — callers handle them however they like.
fn topo_sort_params(mut map: HashMap<String, ParamBody>) -> IndexMap<String, ParamBody> {
    let known: HashSet<String> = map.keys().cloned().collect();

    // Assign stable integer indices in sorted key order for deterministic output.
    let mut names: Vec<String> = map.keys().cloned().collect();
    names.sort();
    let idx: HashMap<&str, usize> =
        names.iter().enumerate().map(|(i, n)| (n.as_str(), i)).collect();
    let n = names.len();

    let mut dependents: Vec<Vec<usize>> = vec![vec![]; n];
    for (name, param) in &map {
        let i = idx[name.as_str()];
        for dep in collect_param_deps(param, &known) {
            let j = idx[dep.as_str()];
            dependents[j].push(i);
        }
    }
    // Sort each adjacency list so newly-ready nodes are enqueued in key order.
    for deps in &mut dependents {
        deps.sort();
    }

    let (order, had_cycle) = crate::util::topological_sort(&dependents);

    let mut result: IndexMap<String, ParamBody> = IndexMap::new();
    for i in order {
        let name = &names[i];
        if let Some(param) = map.remove(name) {
            result.insert(name.clone(), param);
        }
    }
    if had_cycle {
        let mut remaining: Vec<String> = map.keys().cloned().collect();
        remaining.sort();
        for name in remaining {
            if let Some(param) = map.remove(&name) {
                result.insert(name, param);
            }
        }
    }
    result
}

fn topo_sort_postings(
    postings: Vec<Posting>,
    entry_leg_names: &HashSet<String>,
    span: Span,
    entry_name: &str,
    diags: &mut Vec<Diagnostic>,
) -> Vec<Posting> {
    let mut auto_leg: Option<Posting> = None;
    let mut explicit: Vec<Posting> = Vec::new();
    for p in postings {
        if p.amount.is_none() {
            auto_leg = Some(p);
        } else {
            explicit.push(p);
        }
    }

    let n = explicit.len();
    let mut name_to_idx: HashMap<String, usize> = HashMap::new();
    for (i, p) in explicit.iter().enumerate() {
        if let Some(leg) = &p.leg_name {
            name_to_idx.insert(leg.clone(), i);
        }
    }

    let mut dependents: Vec<Vec<usize>> = vec![vec![]; n];
    for (i, p) in explicit.iter().enumerate() {
        if let Some(PostingAmount::Expr(e)) = &p.amount {
            let mut seen: HashSet<usize> = HashSet::new();
            walk_expr(e, &mut |sub| {
                if let Expr::Ref(path) = sub.0.as_ref()
                    && path.0.len() == 1
                    && entry_leg_names.contains(&path.0[0])
                    && let Some(&j) = name_to_idx.get(&path.0[0])
                    && seen.insert(j)
                {
                    dependents[j].push(i);
                }
            });
        }
    }

    let (order, had_cycle) = crate::util::topological_sort(&dependents);

    if had_cycle {
        diags.push(Diagnostic::new(
            span,
            format!("entry `{entry_name}` has a cycle among named legs"),
        ));
        let mut result = explicit;
        if let Some(auto) = auto_leg {
            result.push(auto);
        }
        return result;
    }

    let mut slots: Vec<Option<Posting>> = explicit.into_iter().map(Some).collect();
    let mut sorted: Vec<Posting> = order.into_iter().map(|i| slots[i].take().unwrap()).collect();
    if let Some(auto) = auto_leg {
        sorted.push(auto);
    }
    sorted
}

/// Classify a reference path. Returns None for unknown references.
pub fn resolve_ref(
    path: &Path,
    stocks: &HashSet<Path>,
    params: &HashSet<String>,
) -> Option<RefKind> {
    if stocks.contains(path) {
        return Some(RefKind::Stock(path.clone()));
    }
    if path.0.len() == 1 && params.contains(&path.0[0]) {
        return Some(RefKind::Param(path.0[0].clone()));
    }
    None
}

pub enum RefKind {
    Stock(Path),
    Param(String),
}

fn walk_expr(e: &SpannedExpr, f: &mut impl FnMut(&SpannedExpr)) {
    f(e);
    match e.0.as_ref() {
        Expr::Num(_) | Expr::Bool(_) | Expr::Ref(_) | Expr::ParamAgg(..) => {}
        Expr::Neg(x) => walk_expr(x, f),
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
    }
}

fn collect_fn_call_deps(body: &[Stmt], known: &HashSet<String>) -> Vec<String> {
    let mut deps: Vec<String> = Vec::new();
    let mut visitor = |e: &SpannedExpr| {
        if let Expr::Call(name, _) = e.0.as_ref() && known.contains(name.as_str()) {
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

fn validate_fn_bodies(fns: &HashMap<String, FnDef>, diags: &mut Vec<Diagnostic>) {
    let fn_names: HashSet<String> = fns.keys().cloned().collect();

    for (name, def) in fns {
        let mut visited: HashSet<String> = HashSet::new();
        let mut stack: Vec<String> = collect_fn_call_deps(&def.body, &fn_names);
        while let Some(callee) = stack.pop() {
            if callee == *name {
                diags.push(Diagnostic::new(
                    def.span,
                    format!("function `{name}` contains a recursive call cycle"),
                ));
                break;
            }
            if visited.insert(callee.clone()) {
                if let Some(callee_def) = fns.get(&callee) {
                    stack.extend(collect_fn_call_deps(&callee_def.body, &fn_names));
                }
            }
        }
    }

    for (fn_name, def) in fns {
        let mut scope: HashSet<String> = def.params.iter().cloned().collect();
        for stmt in &def.body {
            match stmt {
                Stmt::Let { name: let_name, value } => {
                    validate_fn_expr(value, &scope, fns, fn_name, def.span, diags);
                    scope.insert(let_name.clone());
                }
                Stmt::Return(expr) => {
                    validate_fn_expr(expr, &scope, fns, fn_name, def.span, diags);
                }
            }
        }
    }
}

fn validate_fn_expr(
    expr: &SpannedExpr,
    scope: &HashSet<String>,
    user_fns: &HashMap<String, FnDef>,
    fn_name: &str,
    fn_span: Span,
    diags: &mut Vec<Diagnostic>,
) {
    walk_expr(expr, &mut |sub: &SpannedExpr| {
        match sub.0.as_ref() {
            Expr::Ref(path) => {
                if !(path.0.len() == 1 && scope.contains(&path.0[0])) {
                    diags.push(Diagnostic::new(
                        sub.1,
                        format!(
                            "unknown reference `{path}` in function `{fn_name}`; \
                             function bodies can only reference their own parameters and local bindings"
                        ),
                    ));
                }
            }
            Expr::Call(callee, args) => {
                if let Some((_, arity)) = BUILTINS.iter().find(|(n, _)| *n == callee.as_str()) {
                    if args.len() != *arity {
                        let word = if *arity == 1 { "argument" } else { "arguments" };
                        diags.push(Diagnostic::new(
                            sub.1,
                            format!("`{callee}` takes {arity} {word}, got {}", args.len()),
                        ));
                    }
                } else if let Some(def) = user_fns.get(callee.as_str()) {
                    if args.len() != def.params.len() {
                        let expected = def.params.len();
                        diags.push(Diagnostic::new(
                            sub.1,
                            format!("`{callee}` takes {expected} argument(s), got {}", args.len()),
                        ));
                    }
                } else {
                    diags.push(Diagnostic::new(
                        sub.1,
                        format!("unknown function `{callee}`"),
                    ));
                }
            }
            Expr::ParamAgg(..) => {
                diags.push(Diagnostic::new(
                    fn_span,
                    format!("function `{fn_name}` cannot use `.ytd`/`.qtd`/`.mtd` aggregations"),
                ));
            }
            _ => {}
        }
    });
}

fn topo_sort_fns(mut map: HashMap<String, FnDef>) -> IndexMap<String, FnDef> {
    let known: HashSet<String> = map.keys().cloned().collect();

    let mut names: Vec<String> = map.keys().cloned().collect();
    names.sort();
    let idx: HashMap<&str, usize> =
        names.iter().enumerate().map(|(i, n)| (n.as_str(), i)).collect();
    let n = names.len();

    let mut dependents: Vec<Vec<usize>> = vec![vec![]; n];
    for (name, def) in &map {
        let i = idx[name.as_str()];
        for dep in collect_fn_call_deps(&def.body, &known) {
            let j = idx[dep.as_str()];
            dependents[j].push(i);
        }
    }
    for deps in &mut dependents {
        deps.sort();
    }

    let (order, had_cycle) = crate::util::topological_sort(&dependents);

    let mut result: IndexMap<String, FnDef> = IndexMap::new();
    for i in order {
        let name = &names[i];
        if let Some(def) = map.remove(name) {
            result.insert(name.clone(), def);
        }
    }
    if had_cycle {
        let mut remaining: Vec<String> = map.keys().cloned().collect();
        remaining.sort();
        for name in remaining {
            if let Some(def) = map.remove(&name) {
                result.insert(name, def);
            }
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    fn parse(src: &str) -> Program {
        let tokens = crate::lexer::lex(src).expect("lex failed");
        crate::parser::parse(tokens).expect("parse failed")
    }

    fn resolve_src(src: &str) -> Result<Model, Vec<Diagnostic>> {
        resolve(&parse(src))
    }

    // --- collect_schedules ---

    #[test]
    fn collect_schedules_deduplicates() {
        let prog = parse("schedule biweekly = every 2 weeks from 2024-01-01\nschedule biweekly = every week");
        let mut r = Resolver::new(&prog);
        r.collect_schedules();
        assert!(r.diags.iter().any(|d| d.message.contains("duplicate schedule")));
    }

    #[test]
    fn collect_schedules_returns_named_schedule() {
        let prog = parse("schedule payday = every month");
        let mut r = Resolver::new(&prog);
        r.collect_schedules();
        assert!(r.diags.is_empty());
        assert!(r.schedules.contains_key("payday"));
    }

    // --- collect_declarations ---

    #[test]
    fn collect_declarations_rejects_unknown_named_schedule() {
        let prog = parse("account Assets:Cash\naccount Liabilities:Loan\nentry payday \"Test\" {\n  Assets:Cash = 100\n  Liabilities:Loan\n}");
        let mut r = Resolver::new(&prog);
        r.collect_schedules();
        r.collect_declarations();
        assert!(r.diags.iter().any(|d| d.message.contains("not defined")));
    }

    #[test]
    fn collect_declarations_aliased_entry_uses_alias_as_key() {
        let prog = parse("account Assets:Cash\naccount Liabilities:Loan\nentry monthly \"Test\" {\n  Assets:Cash = 100\n  Liabilities:Loan\n} as myentry");
        let mut r = Resolver::new(&prog);
        r.collect_schedules();
        r.collect_declarations();
        assert!(r.diags.is_empty(), "{:?}", r.diags);
        assert_eq!(r.entries[0].key, "myentry");
    }

    #[test]
    fn collect_declarations_aliasless_entry_uses_synthetic_key() {
        let prog = parse("account Assets:Cash\naccount Liabilities:Loan\nentry monthly \"Test\" {\n  Assets:Cash = 100\n  Liabilities:Loan\n}");
        let mut r = Resolver::new(&prog);
        r.collect_schedules();
        r.collect_declarations();
        assert!(r.diags.is_empty(), "{:?}", r.diags);
        assert_eq!(r.entries[0].key, "$0");
    }

    #[test]
    fn collect_declarations_duplicate_alias_is_rejected() {
        let prog = parse(
            "account Assets:Cash\naccount Liabilities:Loan\n\
             entry monthly \"A\" { Assets:Cash = 100\n  Liabilities:Loan } as foo\n\
             entry monthly \"B\" { Assets:Cash = 200\n  Liabilities:Loan } as foo",
        );
        let mut r = Resolver::new(&prog);
        r.collect_schedules();
        r.collect_declarations();
        assert!(r.diags.iter().any(|d| d.message.contains("duplicate entry alias")));
    }

    // --- validate_references ---

    #[test]
    fn validate_references_rejects_unknown_account_in_posting() {
        let prog = parse("account Assets:Cash\nentry monthly \"Test\" {\n  Assets:Cash = 100\n  Liabilities:Unknown\n}");
        let mut r = Resolver::new(&prog);
        r.collect_schedules();
        r.collect_declarations();
        r.validate_references();
        assert!(r.diags.iter().any(|d| d.message.contains("unknown account")));
    }

    #[test]
    fn validate_references_rejects_unknown_param_in_expr() {
        let prog = parse("account Assets:Cash\naccount Liabilities:Loan\nentry monthly \"Test\" {\n  Assets:Cash = ghost_param\n  Liabilities:Loan\n}");
        let mut r = Resolver::new(&prog);
        r.collect_schedules();
        r.collect_declarations();
        r.validate_references();
        assert!(r.diags.iter().any(|d| d.message.contains("unknown reference")));
    }

    // --- resolve (end-to-end, existing tests) ---

    #[test]
    fn rejects_nth_schedule_without_from() {
        let src = r#"
            account Assets:Cash
            account Liabilities:Loan
            entry every 2 months "Test" {
                Assets:Cash = 100
                Liabilities:Loan
            }
        "#;
        let errs = resolve_src(src).unwrap_err();
        assert!(
            errs.iter().any(|d| d.message.contains("from")),
            "expected a diagnostic about missing `from`, got: {errs:?}"
        );
    }

    #[test]
    fn accepts_nth_schedule_with_from() {
        let src = r#"
            account Assets:Cash
            account Liabilities:Loan
            entry every 2 months from 2024-01-01 "Test" {
                Assets:Cash = 100
                Liabilities:Loan
            }
        "#;
        assert!(resolve_src(src).is_ok());
    }
}
