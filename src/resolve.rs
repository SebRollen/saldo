use crate::ast::{
    Decl, Expr, ParamBody, Path, Posting, PostingAmount, Program, ScheduleRef, Schedule, Span, SpannedExpr
};
use crate::errors::Diagnostic;
use crate::eval::BUILTINS;
use indexmap::IndexMap;
use std::collections::{HashMap, HashSet, VecDeque};
use crate::ast::schedule::{Every, Period};

#[derive(Debug, Clone)]
pub struct Account {
    pub init: Option<SpannedExpr>,
}

#[derive(Debug)]
pub struct FlowDef {
    pub label: String,
    pub alias: Option<String>,
    pub schedule: Schedule,
    pub postings: Vec<Posting>,
    pub span: Span,
}

impl FlowDef {
    /// The internal identifier used for leg namespacing and cross-flow references.
    /// Equals the alias if present, otherwise the label.
    pub fn key(&self) -> &str {
        self.alias.as_deref().unwrap_or(&self.label)
    }
}

#[derive(Debug)]
pub struct Model {
    /// Accounts in declaration order (used as column order in CSV output).
    pub stocks: IndexMap<Path, Account>,
    pub params: IndexMap<String, ParamBody>,
    pub flows: Vec<FlowDef>,
    pub asserts: Vec<(Schedule, SpannedExpr)>,
    pub leg_names: HashSet<(String, String)>,
}

pub fn resolve(program: &Program) -> Result<Model, Vec<Diagnostic>> {
    let mut stocks: IndexMap<Path, Account> = IndexMap::new();
    let mut schedules: IndexMap<String, Schedule> = IndexMap::new();
    let mut params_map: HashMap<String, ParamBody> = HashMap::new();
    let mut flow_aliases: HashMap<String, Span> = HashMap::new();
    let mut flows: Vec<FlowDef> = Vec::new();
    let mut asserts: Vec<(Schedule, SpannedExpr)> = Vec::new();
    let mut schedule_spans: HashMap<String, Span> = HashMap::new();
    let mut stock_spans: HashMap<Path, Span> = HashMap::new();
    let mut param_spans: HashMap<String, Span> = HashMap::new();
    // (flow_name, leg_name) pairs — legs are namespaced under their flow.
    let mut all_leg_names: HashSet<(String, String)> = HashSet::new();
    let mut diags: Vec<Diagnostic> = Vec::new();

    // Pass 1: collect schedule decls so they can be resolved in next pass
    for (decl, span) in &program.decls {
        if let Decl::Schedule { name, schedule } = decl {
            if let Some(prev) = schedule_spans.get(name) {
                    diags.push(
                        Diagnostic::new(*span, format!("duplicate schedule `{name}`"))
                            .with_note(*prev, "previously declared here"),
                    );

            } else {
                schedule_spans.insert(name.clone(), *span);
                schedules.insert(name.clone(), schedule.clone());
            }
        }
    }
    // Pass 2: collect declarations, enforce unique names.
    for (decl, span) in &program.decls {
        match decl {
            Decl::Account { name, init } => {
                if let Some(prev) = stock_spans.get(name) {
                    diags.push(
                        Diagnostic::new(*span, format!("duplicate account `{name}`"))
                            .with_note(*prev, "previously declared here"),
                    );
                } else {
                    stock_spans.insert(name.clone(), *span);
                    stocks.insert(name.clone(), Account { init: init.clone() });
                }
            }
            Decl::Schedule { .. } => {
                // already processed in previous pass
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
            Decl::Flow {
                label,
                alias,
                schedule,
                postings,
            } => {
                if let Some(a) = alias {
                    if let Some(prev) = flow_aliases.get(a) {
                        diags.push(
                            Diagnostic::new(*span, format!("duplicate flow alias `{a}`"))
                                .with_note(*prev, "previously declared here"),
                        );
                        continue;
                    }
                }
                if postings.is_empty() {
                    diags.push(Diagnostic::new(
                        *span,
                        format!("flow `{label}` has no postings"),
                    ));
                }
                // At most one auto-balance leg.
                let auto_count = postings.iter().filter(|p| p.amount.is_none()).count();
                if auto_count > 1 {
                    diags.push(Diagnostic::new(
                        *span,
                        format!("flow `{label}` has more than one auto-balance posting"),
                    ));
                }

                let key = alias.as_deref().unwrap_or(label.as_str());

                // Collect and validate named legs.
                let mut flow_leg_names: HashSet<String> = HashSet::new();
                for posting in postings {
                    let Some(leg) = &posting.leg_name else { continue };
                    if !flow_leg_names.insert(leg.clone()) {
                        diags.push(Diagnostic::new(
                            *span,
                            format!("duplicate leg name `{leg}` in flow `{label}`"),
                        ));
                    } else if param_spans.contains_key(leg) {
                        diags.push(Diagnostic::new(
                            *span,
                            format!("leg name `{leg}` conflicts with a param of the same name"),
                        ));
                    } else {
                        all_leg_names.insert((key.to_string(), leg.clone()));
                    }
                }

                if let Some(a) = alias {
                    flow_aliases.insert(a.clone(), *span);
                }
                let schedule = match schedule {
                    ScheduleRef::Literal(s) => s,
                    ScheduleRef::Named(n) => {
                        match schedules.get(n) {
                            Some(s) => s,
                            None => {
                                diags.push(Diagnostic::new(
                                        *span,
                                        format!("schedule `{n}` is not defined")
                                ));
                                continue;
                            }
                        }
                    }
                };
                flows.push(FlowDef {
                    label: label.clone(),
                    alias: alias.clone(),
                    schedule: schedule.clone(),
                    postings: topo_sort_postings(postings.clone(), &flow_leg_names, *span, label, &mut diags),
                    span: *span,
                });
            }
            Decl::Assert(sched, e) => {
                let schedule = match sched {
                    None => {
                        // Default to daily check
                        Schedule::Every(Every { period: Period::Day, nth: None, start: None })
                    },
                    Some(ScheduleRef::Literal(s)) => s.clone(),
                    Some(ScheduleRef::Named(n)) => {
                        match schedules.get(n) {
                            Some(s) => s.clone(),
                            None => {
                                diags.push(Diagnostic::new(
                                        *span,
                                        format!("schedule `{n}` is not defined")
                                ));
                                continue;
                            }
                        }
                    }
                };
                asserts.push((schedule, e.clone()))
            },
        }
    }

    // Pass 3: reference checks.
    let stock_set: HashSet<Path> = stocks.keys().cloned().collect();
    let param_set: HashSet<String> = params_map.keys().cloned().collect();

    let check_path_is_stock = |p: &Path, span: Span, diags: &mut Vec<Diagnostic>| {
        if !stock_set.contains(p) {
            diags.push(Diagnostic::new(span, format!("unknown account `{p}`")));
        }
    };

    // flow_context: name of the enclosing flow (if any), used to validate ParamAgg references.
    // extra_refs: bare leg names valid inside the current flow's posting expressions.
    let check_expr = |e: &SpannedExpr,
                      diags: &mut Vec<Diagnostic>,
                      flow_context: Option<&str>,
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
            if let Expr::ParamAgg(flow_opt, leg, _) = sub.0.as_ref() {
                let key: (String, String) = match flow_opt {
                    Some(flow) => (flow.clone(), leg.clone()),
                    None => match flow_context {
                        Some(f) => (f.to_string(), leg.clone()),
                        None => {
                            diags.push(Diagnostic::new(
                                sub.1,
                                format!("unqualified leg `{leg}` — use `<flow>.{leg}.ytd` outside of a flow"),
                            ));
                            return;
                        }
                    },
                };
                if !all_leg_names.contains(&key) {
                    diags.push(Diagnostic::new(
                        sub.1,
                        format!("unknown named leg `{}.{}`", key.0, key.1),
                    ));
                }
            }
        });
    };
    let no_extra: HashSet<String> = HashSet::new();

    for (decl, span) in &program.decls {
        match decl {
            Decl::Account { init: Some(e), .. } => check_expr(e, &mut diags, None, &no_extra),
            Decl::Param { body, .. } => match body {
                ParamBody::Const(e) => check_expr(e, &mut diags, None, &no_extra),
                ParamBody::Schedule(intervals) => {
                    for i in intervals {
                        check_expr(&i.value, &mut diags, None, &no_extra);
                    }
                }
            },
            Decl::Flow { label, alias, postings, .. } => {
                let key = alias.as_deref().unwrap_or(label.as_str());
                let flow_legs: HashSet<String> = postings
                    .iter()
                    .filter_map(|p| p.leg_name.as_ref())
                    .cloned()
                    .collect();
                for posting in postings {
                    check_path_is_stock(&posting.account, *span, &mut diags);
                    if let Some(PostingAmount::Expr(e)) = &posting.amount {
                        check_expr(e, &mut diags, Some(key), &flow_legs);
                    }
                }
            }
            Decl::Assert(_, e) => check_expr(e, &mut diags, None, &no_extra),
            _ => {}
        }
    }

    if diags.is_empty() {
        Ok(Model {
            stocks,
            params: topo_sort_params(params_map),
            flows,
            asserts,
            leg_names: all_leg_names,
        })
    } else {
        Err(diags)
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
    deps.dedup();
    deps
}

fn topo_sort_params(mut map: HashMap<String, ParamBody>) -> IndexMap<String, ParamBody> {
    let known: HashSet<String> = map.keys().cloned().collect();
    let mut in_deg: HashMap<String, usize> = map.keys().map(|n| (n.clone(), 0)).collect();
    let mut dependents: HashMap<String, Vec<String>> =
        map.keys().map(|n| (n.clone(), vec![])).collect();

    for (name, param) in &map {
        for dep in collect_param_deps(param, &known) {
            dependents.entry(dep).or_default().push(name.clone());
            *in_deg.entry(name.clone()).or_insert(0) += 1;
        }
    }

    let mut queue: Vec<String> = in_deg
        .iter()
        .filter(|&(_, &d)| d == 0)
        .map(|(n, _)| n.clone())
        .collect();
    queue.sort();
    let mut queue: VecDeque<String> = queue.into();

    let mut result: IndexMap<String, ParamBody> = IndexMap::new();
    while let Some(name) = queue.pop_front() {
        if let Some(param) = map.remove(&name) {
            result.insert(name.clone(), param);
        }
        if let Some(deps) = dependents.get(&name) {
            let mut next: Vec<String> = deps
                .iter()
                .filter_map(|d| {
                    let deg = in_deg.get_mut(d)?;
                    *deg -= 1;
                    (*deg == 0).then(|| d.clone())
                })
            .collect();
            next.sort();
            queue.extend(next);
        }
    }
    let mut remaining: Vec<String> = map.keys().cloned().collect();
    remaining.sort();
    for name in remaining {
        if let Some(param) = map.remove(&name) {
            result.insert(name, param);
        }
    }
    result
}

fn topo_sort_postings(
    postings: Vec<Posting>,
    flow_leg_names: &HashSet<String>,
    span: Span,
    flow_name: &str,
    diags: &mut Vec<Diagnostic>,
) -> Vec<Posting> {
    // Auto-balance posting (no amount) always goes last.
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

    let mut in_deg: Vec<usize> = vec![0; n];
    let mut dependents: Vec<Vec<usize>> = vec![vec![]; n];

    for (i, p) in explicit.iter().enumerate() {
        if let Some(PostingAmount::Expr(e)) = &p.amount {
            let mut seen: HashSet<usize> = HashSet::new();
            walk_expr(e, &mut |sub| {
                if let Expr::Ref(path) = sub.0.as_ref()
                    && path.0.len() == 1
                    && flow_leg_names.contains(&path.0[0])
                    && let Some(&j) = name_to_idx.get(&path.0[0])
                    && seen.insert(j) {
                            dependents[j].push(i);
                            in_deg[i] += 1;
                        }
            });
        }
    }

    let mut queue: VecDeque<usize> = (0..n).filter(|&i| in_deg[i] == 0).collect();
    let mut order: Vec<usize> = Vec::with_capacity(n);
    while let Some(i) = queue.pop_front() {
        order.push(i);
        for &j in &dependents[i] {
            in_deg[j] -= 1;
            if in_deg[j] == 0 {
                queue.push_back(j);
            }
        }
    }

    if order.len() != n {
        diags.push(Diagnostic::new(
            span,
            format!("flow `{flow_name}` has a cycle among named legs"),
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
