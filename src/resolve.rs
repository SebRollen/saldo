use crate::ast::{
    Decl, Expr, ParamBody, Path, Posting, PostingAmount, Program, ScheduleRef, Schedule, Span, SpannedExpr
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


#[derive(Debug)]
pub struct Model {
    /// Accounts in declaration order (used as column order in CSV output).
    pub stocks: IndexMap<Path, Account>,
    pub params: IndexMap<String, ParamBody>,
    pub entries: Vec<EntryDef>,
    pub asserts: Vec<(Schedule, SpannedExpr)>,
    pub leg_names: HashSet<(String, String)>,
}

/// Intermediate output of Pass 2, consumed by Pass 3 and converted to Model.
struct Declarations {
    stocks:    IndexMap<Path, Account>,
    params:    HashMap<String, ParamBody>, // un-sorted; topo-sorted in into_model()
    entries:   Vec<EntryDef>,
    asserts:   Vec<(Schedule, SpannedExpr)>,
    leg_names: HashSet<(String, String)>,
}

impl Declarations {
    fn into_model(self) -> Model {
        Model {
            stocks:    self.stocks,
            params:    topo_sort_params(self.params),
            entries:   self.entries,
            asserts:   self.asserts,
            leg_names: self.leg_names,
        }
    }
}

pub fn resolve(program: &Program) -> Result<Model, Vec<Diagnostic>> {
    let mut diags = Vec::new();
    let schedules = collect_schedules(program, &mut diags);
    let decls     = collect_declarations(program, &schedules, &mut diags);
    validate_references(&decls, program, &mut diags);
    if diags.is_empty() {
        Ok(decls.into_model())
    } else {
        Err(diags)
    }
}

fn collect_schedules(
    program: &Program,
    diags: &mut Vec<Diagnostic>,
) -> IndexMap<String, Schedule> {
    let mut schedules: IndexMap<String, Schedule> = IndexMap::new();
    let mut schedule_spans: HashMap<String, Span> = HashMap::new();
    for (decl, span) in &program.decls {
        if let Decl::Schedule { name, schedule } = decl {
            if let Some(prev) = schedule_spans.get(name) {
                diags.push(
                    Diagnostic::new(*span, format!("duplicate schedule `{name}`"))
                        .with_note(*prev, "previously declared here"),
                );
            } else {
                check_periodic_schedule(schedule, *span, diags);
                schedule_spans.insert(name.clone(), *span);
                schedules.insert(name.clone(), schedule.clone());
            }
        }
    }
    schedules
}

fn collect_declarations(
    program: &Program,
    schedules: &IndexMap<String, Schedule>,
    diags: &mut Vec<Diagnostic>,
) -> Declarations {
    let mut stocks: IndexMap<Path, Account> = IndexMap::new();
    let mut params_map: HashMap<String, ParamBody> = HashMap::new();
    let mut entries: Vec<EntryDef> = Vec::new();
    let mut asserts: Vec<(Schedule, SpannedExpr)> = Vec::new();
    let mut all_leg_names: HashSet<(String, String)> = HashSet::new();

    // dup-detection maps, local to this pass
    let mut stock_spans: HashMap<Path, Span> = HashMap::new();
    let mut param_spans: HashMap<String, Span> = HashMap::new();
    let mut entry_aliases: HashMap<String, Span> = HashMap::new();

    for (decl, span) in &program.decls {
        match decl {
            Decl::Account { name, opening } => {
                if let Some(prev) = stock_spans.get(name) {
                    diags.push(
                        Diagnostic::new(*span, format!("duplicate account `{name}`"))
                            .with_note(*prev, "previously declared here"),
                    );
                } else {
                    stock_spans.insert(name.clone(), *span);
                    stocks.insert(name.clone(), Account { opening: opening.clone() });
                }
            }
            Decl::Schedule { .. } => {
                // already processed in collect_schedules
            }
            Decl::Param { name, body, .. } => {
                if let Some(prev) = param_spans.get(name) {
                    diags.push(
                        Diagnostic::new(*span, format!("duplicate param `{name}`"))
                            .with_note(*prev, "previously declared here"),
                    );
                    continue;
                }
                match body {
                    ParamBody::Const(e) => {
                        params_map.insert(name.clone(), ParamBody::Const(e.clone()));
                    }
                    ParamBody::Schedule(intervals) => {
                        let mut sorted = intervals.clone();
                        sorted.sort_by_key(|i| i.from);
                        for win in sorted.windows(2) {
                            let a = &win[0];
                            let b = &win[1];
                            let a_end = a.to.unwrap_or(chrono::NaiveDate::MAX);
                            if a_end > b.from {
                                diags.push(Diagnostic::new(
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
                                diags.push(Diagnostic::new(
                                    *span,
                                    format!("param `{name}` interval ends before it starts"),
                                ));
                            }
                        }
                        params_map.insert(name.clone(), ParamBody::Schedule(sorted));
                    }
                }
                param_spans.insert(name.clone(), *span);
            }
            Decl::Entry {
                label,
                alias,
                schedule,
                postings,
            } => {
                if let Some(a) = alias {
                    if let Some(prev) = entry_aliases.get(a) {
                        diags.push(
                            Diagnostic::new(*span, format!("duplicate entry alias `{a}`"))
                                .with_note(*prev, "previously declared here"),
                        );
                        continue;
                    }
                }
                if postings.is_empty() {
                    diags.push(Diagnostic::new(
                        *span,
                        format!("entry `{label}` has no postings"),
                    ));
                }
                let auto_count = postings.iter().filter(|p| p.amount.is_none()).count();
                if auto_count > 1 {
                    diags.push(Diagnostic::new(
                        *span,
                        format!("entry `{label}` has more than one auto-balance posting"),
                    ));
                }

                let key = alias.clone().unwrap_or_else(|| format!("${}", entries.len()));

                let mut entry_leg_names: HashSet<String> = HashSet::new();
                for posting in postings {
                    let Some(leg) = &posting.leg_name else { continue };
                    if !entry_leg_names.insert(leg.clone()) {
                        diags.push(Diagnostic::new(
                            *span,
                            format!("duplicate leg name `{leg}` in entry `{label}`"),
                        ));
                    } else if param_spans.contains_key(leg) {
                        diags.push(Diagnostic::new(
                            *span,
                            format!("leg name `{leg}` conflicts with a param of the same name"),
                        ));
                    } else {
                        all_leg_names.insert((key.clone(), leg.clone()));
                    }
                }

                if let Some(a) = alias {
                    entry_aliases.insert(a.clone(), *span);
                }
                let schedule = match schedule {
                    ScheduleRef::Literal(s) => s,
                    ScheduleRef::Named(n) => {
                        match schedules.get(n) {
                            Some(s) => s,
                            None => {
                                diags.push(Diagnostic::new(
                                    *span,
                                    format!("schedule `{n}` is not defined"),
                                ));
                                continue;
                            }
                        }
                    }
                };
                check_periodic_schedule(schedule, *span, diags);
                entries.push(EntryDef {
                    label: label.clone(),
                    key,
                    schedule: schedule.clone(),
                    postings: topo_sort_postings(postings.clone(), &entry_leg_names, *span, label, diags),
                    span: *span,
                });
            }
            Decl::Assert{schedule, asserted} => {
                let schedule = match schedule {
                    None => Schedule::Periodic(Periodic { period: Period::Day, nth: None, start: None }),
                    Some(ScheduleRef::Literal(s)) => s.clone(),
                    Some(ScheduleRef::Named(n)) => {
                        match schedules.get(n) {
                            Some(s) => s.clone(),
                            None => {
                                diags.push(Diagnostic::new(
                                    *span,
                                    format!("schedule `{n}` is not defined"),
                                ));
                                continue;
                            }
                        }
                    }
                };
                check_periodic_schedule(&schedule, *span, diags);
                asserts.push((schedule, asserted.clone()));
            }
        }
    }

    Declarations { stocks, params: params_map, entries, asserts, leg_names: all_leg_names }
}

fn validate_references(
    decls: &Declarations,
    program: &Program,
    diags: &mut Vec<Diagnostic>,
) {
    let stock_set: HashSet<Path> = decls.stocks.keys().cloned().collect();
    let param_set: HashSet<String> = decls.params.keys().cloned().collect();

    let check_path_is_stock = |p: &Path, span: Span, diags: &mut Vec<Diagnostic>| {
        if !stock_set.contains(p) {
            diags.push(Diagnostic::new(span, format!("unknown account `{p}`")));
        }
    };

    // entry_context: key of the enclosing entry (if any), for validating ParamAgg refs.
    // extra_refs: bare leg names valid inside the current entry's posting expressions.
    let check_expr = |e: &SpannedExpr,
                      diags: &mut Vec<Diagnostic>,
                      entry_context: Option<&str>,
                      extra_refs: &HashSet<String>| {
        walk_expr(e, &mut |sub: &SpannedExpr| {
            if let Expr::Ref(path) = sub.0.as_ref()
                && resolve_ref(path, &stock_set, &param_set).is_none()
                && !(path.0.len() == 1 && extra_refs.contains(&path.0[0]))
            {
                diags.push(Diagnostic::new(
                    sub.1,
                    format!("unknown reference `{path}`"),
                ));
            }
            if let Expr::Call(name, args) = sub.0.as_ref() {
                match BUILTINS.iter().find(|(n, _)| *n == name.as_str()) {
                    None => diags.push(Diagnostic::new(
                        sub.1,
                        format!("unknown function `{name}`"),
                    )),
                    Some((_, arity)) if args.len() != *arity => {
                        let argument_str = if *arity == 1 { "argument" } else { "arguments" };
                        diags.push(Diagnostic::new(
                            sub.1,
                            format!("`{name}` takes {arity} {argument_str}, got {}", args.len()),
                        ))
                    }
                    _ => {}
                }
            }
            if let Expr::ParamAgg(entry_opt, leg, _) = sub.0.as_ref() {
                let key: (String, String) = match entry_opt {
                    Some(entry) => (entry.clone(), leg.clone()),
                    None => match entry_context {
                        Some(e) => (e.to_string(), leg.clone()),
                        None => {
                            diags.push(Diagnostic::new(
                                sub.1,
                                format!("unqualified leg `{leg}` — use `<entry>.{leg}.ytd` outside of an entry"),
                            ));
                            return;
                        }
                    },
                };
                if !decls.leg_names.contains(&key) {
                    diags.push(Diagnostic::new(
                        sub.1,
                        format!("unknown named leg `{}.{}`", key.0, key.1),
                    ));
                }
            }
        });
    };
    let no_extra: HashSet<String> = HashSet::new();

    for (decl, _span) in &program.decls {
        match decl {
            Decl::Account { opening: Some((e, _)), .. } => check_expr(e, diags, None, &no_extra),
            Decl::Param { body, .. } => match body {
                ParamBody::Const(e) => check_expr(e, diags, None, &no_extra),
                ParamBody::Schedule(intervals) => {
                    for i in intervals {
                        check_expr(&i.value, diags, None, &no_extra);
                    }
                }
            },
            Decl::Assert{ asserted, .. } => check_expr(asserted, diags, None, &no_extra),
            _ => {}
        }
    }
    for entry in &decls.entries {
        let entry_legs: HashSet<String> = entry.postings
            .iter()
            .filter_map(|p| p.leg_name.as_ref())
            .cloned()
            .collect();
        for posting in &entry.postings {
            check_path_is_stock(&posting.account, entry.span, diags);
            if let Some(PostingAmount::Expr(e)) = &posting.amount {
                check_expr(e, diags, Some(&entry.key), &entry_legs);
            }
        }
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lexer::Lexer;
    use crate::parser::Parser;

    fn parse(src: &str) -> Program {
        let tokens = Lexer::new(src).lex().expect("lex failed");
        Parser::new(tokens).parse().expect("parse failed")
    }

    fn resolve_src(src: &str) -> Result<Model, Vec<Diagnostic>> {
        resolve(&parse(src))
    }

    // --- collect_schedules ---

    #[test]
    fn collect_schedules_deduplicates() {
        let prog = parse("schedule biweekly = every 2 weeks from 2024-01-01\nschedule biweekly = every week");
        let mut diags = Vec::new();
        collect_schedules(&prog, &mut diags);
        assert!(diags.iter().any(|d| d.message.contains("duplicate schedule")));
    }

    #[test]
    fn collect_schedules_returns_named_schedule() {
        let prog = parse("schedule payday = every month");
        let mut diags = Vec::new();
        let schedules = collect_schedules(&prog, &mut diags);
        assert!(diags.is_empty());
        assert!(schedules.contains_key("payday"));
    }

    // --- collect_declarations ---

    #[test]
    fn collect_declarations_rejects_unknown_named_schedule() {
        let prog = parse("account Assets:Cash\naccount Liabilities:Loan\nentry payday \"Test\" {\n  Assets:Cash = 100\n  Liabilities:Loan\n}");
        let mut diags = Vec::new();
        let schedules = collect_schedules(&prog, &mut diags);
        collect_declarations(&prog, &schedules, &mut diags);
        assert!(diags.iter().any(|d| d.message.contains("not defined")));
    }

    #[test]
    fn collect_declarations_aliased_entry_uses_alias_as_key() {
        let prog = parse("account Assets:Cash\naccount Liabilities:Loan\nentry monthly \"Test\" {\n  Assets:Cash = 100\n  Liabilities:Loan\n} as myentry");
        let mut diags = Vec::new();
        let schedules = collect_schedules(&prog, &mut diags);
        let decls = collect_declarations(&prog, &schedules, &mut diags);
        assert!(diags.is_empty(), "{diags:?}");
        assert_eq!(decls.entries[0].key, "myentry");
    }

    #[test]
    fn collect_declarations_aliasless_entry_uses_synthetic_key() {
        let prog = parse("account Assets:Cash\naccount Liabilities:Loan\nentry monthly \"Test\" {\n  Assets:Cash = 100\n  Liabilities:Loan\n}");
        let mut diags = Vec::new();
        let schedules = collect_schedules(&prog, &mut diags);
        let decls = collect_declarations(&prog, &schedules, &mut diags);
        assert!(diags.is_empty(), "{diags:?}");
        assert_eq!(decls.entries[0].key, "$0");
    }

    #[test]
    fn collect_declarations_duplicate_alias_is_rejected() {
        let prog = parse(
            "account Assets:Cash\naccount Liabilities:Loan\n\
             entry monthly \"A\" { Assets:Cash = 100\n  Liabilities:Loan } as foo\n\
             entry monthly \"B\" { Assets:Cash = 200\n  Liabilities:Loan } as foo",
        );
        let mut diags = Vec::new();
        let schedules = collect_schedules(&prog, &mut diags);
        collect_declarations(&prog, &schedules, &mut diags);
        assert!(diags.iter().any(|d| d.message.contains("duplicate entry alias")));
    }

    // --- validate_references ---

    #[test]
    fn validate_references_rejects_unknown_account_in_posting() {
        let prog = parse("account Assets:Cash\nentry monthly \"Test\" {\n  Assets:Cash = 100\n  Liabilities:Unknown\n}");
        let mut diags = Vec::new();
        let schedules = collect_schedules(&prog, &mut diags);
        let decls = collect_declarations(&prog, &schedules, &mut diags);
        validate_references(&decls, &prog, &mut diags);
        assert!(diags.iter().any(|d| d.message.contains("unknown account")));
    }

    #[test]
    fn validate_references_rejects_unknown_param_in_expr() {
        let prog = parse("account Assets:Cash\naccount Liabilities:Loan\nentry monthly \"Test\" {\n  Assets:Cash = ghost_param\n  Liabilities:Loan\n}");
        let mut diags = Vec::new();
        let schedules = collect_schedules(&prog, &mut diags);
        let decls = collect_declarations(&prog, &schedules, &mut diags);
        validate_references(&decls, &prog, &mut diags);
        assert!(diags.iter().any(|d| d.message.contains("unknown reference")));
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
