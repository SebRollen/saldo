use crate::ast::{AggKind, BinOp, Expr, ParamBody, Path, PostingAmount, Schedule, Span, SpannedExpr, Stmt};
use crate::errors::Diagnostic;
use crate::resolver::{resolve_ref, FnDef, Model, RefKind};
use crate::unit::Unit;
use chrono::{Datelike, Duration, NaiveDate};
use indexmap::IndexMap;
use rust_decimal::Decimal;
use std::collections::{HashMap, HashSet};
use std::fmt;

#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Num(Decimal, Unit),
    Bool(bool),
}

impl fmt::Display for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Value::Num(n, unit) if unit.is_scalar() => write!(f, "{n}"),
            Value::Num(n, unit) => write!(f, "{n} {unit}"),
            Value::Bool(b) => write!(f, "{b}"),
        }
    }
}

#[derive(Debug)]
pub struct SimLog {
    pub transactions: Vec<Transaction>,
    pub snapshots: Vec<DaySnapshot>,
    pub opening: IndexMap<Path, Decimal>,
    pub account_currencies: IndexMap<Path, Unit>,
}

#[derive(Debug)]
pub struct Transaction {
    pub date: NaiveDate,
    pub label: String,
    pub postings: Vec<(Path, Decimal, Unit)>,
}

#[derive(Debug)]
pub struct DaySnapshot {
    pub date: NaiveDate,
    pub balances: IndexMap<Path, Decimal>,
}

struct Environment<'m> {
    /// Balance per account (amount only; currency is in `account_currencies`).
    stocks: HashMap<Path, Decimal>,
    /// Declared currency per account, looked up when producing untyped postings.
    account_currencies: HashMap<Path, Unit>,
    params: HashMap<String, Value>,
    /// (flow_name, leg_name) → value for the current day (0 on non-firing days).
    leg_values: HashMap<(&'m str, &'m str), Value>,
    /// ((flow_name, leg_name), period) → running total.
    accumulators: HashMap<((&'m str, &'m str), AggKind), Value>,
    stock_set: HashSet<Path>,
    param_set: HashSet<String>,
    /// All declared (flow_name, leg_name) pairs.
    leg_set: HashSet<(&'m str, &'m str)>,
    /// Name and schedule of the entry currently being evaluated.
    current_entry: Option<&'m str>,
    current_entry_schedule: Option<&'m Schedule>,
    /// Opening dates for accounts that have them (used for pre-opening error checks).
    opening_dates: HashMap<Path, NaiveDate>,
    /// The current simulation date (set at the top of each day's loop iteration).
    current_date: NaiveDate,
    /// User-defined functions, cloned from the model at construction time.
    fns: HashMap<String, FnDef>,
}

impl<'m> Environment<'m> {
    fn new(model: &'m Model) -> Self {
        let stock_set = model.stocks.keys().cloned().collect();
        let param_set = model.params.keys().cloned().collect();
        let leg_set = model.leg_names.iter().map(|(a, b)| (a.as_str(), b.as_str())).collect();
        let opening_dates = model.stocks.iter()
            .filter_map(|(p, a)| a.opening.as_ref().map(|(_, d)| (p.clone(), *d)))
            .collect();
        let account_currencies = model.stocks.iter()
            .map(|(p, a)| (p.clone(), a.currency.clone()))
            .collect();
        let fns = model.fns.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
        Self {
            stocks: HashMap::new(),
            account_currencies,
            params: HashMap::new(),
            leg_values: HashMap::new(),
            accumulators: HashMap::new(),
            stock_set,
            param_set,
            leg_set,
            current_entry: None,
            current_entry_schedule: None,
            opening_dates,
            current_date: NaiveDate::MIN,
            fns,
        }
    }

    fn add_stock(&mut self, name: Path, value: Decimal) {
        self.stocks.insert(name, value);
    }

    fn add_param(&mut self, name: String, value: Value) {
        self.params.insert(name, value);
    }

    fn stocks_mut(&mut self) -> &mut HashMap<Path, Decimal> {
        &mut self.stocks
    }

    fn advance_period(&mut self) {
        // Accumulate named leg values into period totals. Runs every day; since leg_values
        // is cleared at the start of each day and only populated when a flow fires, this
        // is a no-op on non-firing days and accumulates the actual amount on firing days.
        let items: Vec<_> = self.leg_values.iter()
            .filter_map(|(&k, v)| {
                if let Value::Num(n, u) = v { Some((k, *n, u.clone())) } else { None }
            })
            .collect();
        for (key, amount, unit) in items {
            for kind in [AggKind::Mtd, AggKind::Qtd, AggKind::Ytd] {
                let acc = self.accumulators
                    .entry((key, kind))
                    .or_insert_with(|| Value::Num(Decimal::ZERO, unit.clone()));
                if let Value::Num(acc_amount, acc_unit) = acc {
                    *acc_amount += amount;
                    // Adopt the incoming unit when the accumulator is still zero-scalar
                    // (i.e., before any firings have populated it).
                    if acc_unit.is_scalar() && !unit.is_scalar() {
                        *acc_unit = unit.clone();
                    }
                }
            }
        }
    }

    fn reset_periods(&mut self, t: NaiveDate) {
        if t.day() == 1 {
            self.accumulators.retain(|(_, k), _| *k != AggKind::Mtd);
        }
        if t.day() == 1 && matches!(t.month(), 1 | 4 | 7 | 10) {
            self.accumulators.retain(|(_, k), _| *k != AggKind::Qtd);
        }
        if t.month() == 1 && t.day() == 1 {
            self.accumulators.retain(|(_, k), _| *k != AggKind::Ytd);
        }
    }
}

impl Model {
    pub fn simulate(&self, start: NaiveDate, end: NaiveDate) -> Result<SimLog, Diagnostic> {
        let mut env = Environment::new(self);

        // Accounts with no opening date are always available, starting at zero.
        for (name, account) in &self.stocks {
            if account.opening.is_none() {
                env.add_stock(name.clone(), Decimal::ZERO);
            }
        }

        // Simulate from the earliest opening date (if before start) so the
        // warmup period accumulates the right balances before reporting starts.
        let effective_start = self.stocks.values()
            .filter_map(|a| a.opening.as_ref().map(|(_, d)| *d))
            .min()
            .map(|earliest| earliest.min(start))
            .unwrap_or(start);

        let mut log = SimLog {
            transactions: Vec::new(),
            snapshots: Vec::new(),
            opening: IndexMap::new(),
            account_currencies: self.stocks.iter()
                .map(|(p, a)| (p.clone(), a.currency.clone()))
                .collect(),
        };

        let mut t = effective_start;
        let mut opening_captured = false;
        while t <= end {
            env.current_date = t;

            // Initialize accounts whose opening date is today.
            for (name, account) in &self.stocks {
                if let Some((expr, date)) = &account.opening {
                    if *date == t {
                        let v = eval_num(expr, &env)?;
                        env.add_stock(name.clone(), v);
                    }
                }
            }

            // Capture opening balances at the start of the user's simulation range,
            // after any accounts that open today are initialized but before entries fire.
            if t == start && !opening_captured {
                log.opening = self.stocks.keys()
                    .map(|p| (p.clone(), *env.stocks.get(p).unwrap_or(&Decimal::ZERO)))
                    .collect();
                opening_captured = true;
            }

            env.leg_values.clear();
            env.reset_periods(t);
            self.evaluate_params(t, &mut env)?;
            let txs = self.apply_flows(t, &mut env)?;
            self.check_assertions(t, &env)?;

            if t >= start {
                log.transactions.extend(txs);
                let balances: IndexMap<Path, Decimal> = self
                    .stocks
                    .keys()
                    .map(|p| (p.clone(), *env.stocks.get(p).unwrap_or(&Decimal::ZERO)))
                    .collect();
                log.snapshots.push(DaySnapshot { date: t, balances });
            }

            env.advance_period();

            t = t
                .checked_add_signed(Duration::days(1))
                .ok_or_else(|| Diagnostic::new((0..0).into(), "date overflow"))?;
        }

        if !opening_captured {
            log.opening = self.stocks.keys()
                .map(|p| (p.clone(), *env.stocks.get(p).unwrap_or(&Decimal::ZERO)))
                .collect();
        }

        Ok(log)
    }

    fn evaluate_params<'m>(
        &'m self,
        t: NaiveDate,
        env: &mut Environment<'m>,
    ) -> Result<(), Diagnostic> {
        for (name, def) in &self.params {
            match &def.body {
                ParamBody::Const(e) => {
                    let Value::Num(n, expr_unit) = eval_expr(e, env)? else {
                        return Err(Diagnostic::new(e.1, "param must evaluate to a number"));
                    };
                    // Explicit annotation overrides a bare scalar literal's unit;
                    // if the expression already carries a unit, keep it.
                    let unit = if !def.unit.is_scalar() { def.unit.clone() } else { expr_unit };
                    env.add_param(name.clone(), Value::Num(n, unit));
                }
                ParamBody::Schedule(intervals) => {
                    if let Some(iv) = intervals.iter().find(|iv| iv.contains(t)) {
                        let Value::Num(n, expr_unit) = eval_expr(&iv.value, env)? else {
                            return Err(Diagnostic::new(iv.value.1, "param must evaluate to a number"));
                        };
                        let unit = if !def.unit.is_scalar() { def.unit.clone() } else { expr_unit };
                        env.add_param(name.clone(), Value::Num(n, unit));
                    }
                }
            }
        }
        Ok(())
    }

    fn apply_flows<'m>(
        &'m self,
        t: NaiveDate,
        env: &mut Environment<'m>,
    ) -> Result<Vec<Transaction>, Diagnostic> {
        let mut txs = Vec::new();
        for entry in &self.entries {
            if !entry.schedule.matches(t) {
                continue;
            }
            env.current_entry = Some(entry.key.as_str());
            env.current_entry_schedule = Some(&entry.schedule);
            let mut explicit: Vec<(Path, Decimal, Unit)> = Vec::new();
            let mut auto_leg: Option<(Path, Option<&'m str>)> = None;

            for posting in &entry.postings {
                check_account_open(env, &posting.account, entry.span)?;
                match &posting.amount {
                    Some(PostingAmount::Expr(e)) => {
                        let val = eval_expr(e, env).map_err(|d| {
                            d.with_note(entry.span, format!("in entry `{}`", entry.label))
                        })?;
                        let Value::Num(raw_amt, unit) = val else {
                            return Err(Diagnostic::new(e.1, "posting amount must be numeric")
                                .with_note(entry.span, format!("in entry `{}`", entry.label)));
                        };
                        let amt = raw_amt.round_dp(2);
                        if let Some(leg) = &posting.leg_name {
                            env.leg_values
                                .insert((entry.key.as_str(), leg.as_str()), Value::Num(amt, unit.clone()));
                        }
                        explicit.push((posting.account.clone(), amt, unit));
                    }
                    Some(PostingAmount::All) => {
                        // Clear the current balance: amt = -(current balance).
                        let current = *env.stocks.get(&posting.account).unwrap_or(&Decimal::ZERO);
                        let amt = -current.round_dp(2);
                        let currency = env.account_currencies
                            .get(&posting.account)
                            .cloned()
                            .unwrap_or_default();
                        if let Some(leg) = &posting.leg_name {
                            env.leg_values
                                .insert((entry.key.as_str(), leg.as_str()), Value::Num(amt, currency.clone()));
                        }
                        explicit.push((posting.account.clone(), amt, currency));
                    }
                    None => {
                        auto_leg = Some((posting.account.clone(), posting.leg_name.as_deref()));
                    }
                }
            }

            let explicit_sum: Decimal = explicit.iter().map(|(_, c, _)| c).sum();

            // Apply explicit postings to balances.
            for (account, amt, _) in &explicit {
                *env.stocks_mut()
                    .entry(account.clone())
                    .or_insert(Decimal::ZERO) += amt;
            }

            // Auto-balance posting: exact negation guarantees the transaction sums to zero.
            let mut postings = explicit;
            if let Some((account, leg_name)) = auto_leg {
                let auto = -explicit_sum;
                // Derive the auto-balance currency from the explicit postings (first non-scalar),
                // falling back to the account's declared currency.
                let auto_currency = postings.iter()
                    .find_map(|(_, _, u)| if !u.is_scalar() { Some(u.clone()) } else { None })
                    .or_else(|| env.account_currencies.get(&account).cloned())
                    .unwrap_or_default();
                if let Some(leg) = leg_name {
                    env.leg_values.insert((entry.key.as_str(), leg), Value::Num(auto, auto_currency.clone()));
                }
                *env.stocks_mut()
                    .entry(account.clone())
                    .or_insert(Decimal::ZERO) += auto;
                postings.push((account, auto, auto_currency));
            }

            txs.push(Transaction {
                date: t,
                label: entry.label.clone(),
                postings,
            });
        }
        env.current_entry = None;
        env.current_entry_schedule = None;
        Ok(txs)
    }

    fn check_assertions<'m>(&'m self, t: NaiveDate, env: &Environment<'m>) -> Result<(), Diagnostic> {
        for (sched, expr) in &self.asserts {
            if !sched.matches(t) {
                continue;
            }
            let span = expr.1;
            match eval_expr(expr, env)? {
                Value::Bool(true) => {}
                Value::Bool(false) => {
                    let msg = if let Expr::Bin(lhs, op, rhs) = expr.0.as_ref() {
                        if let (Ok(lv), Ok(rv)) = (eval_expr(lhs, env), eval_expr(rhs, env)) {
                            format!("assertion failed on {t}: {lv} {op} {rv}")
                        } else {
                            format!("assertion failed on {t}")
                        }
                    } else {
                        format!("assertion failed on {t}")
                    };
                    return Err(Diagnostic::new(span, msg));
                }
                Value::Num(_, _) => {
                    return Err(Diagnostic::new(
                        span,
                        "assertion expression must evaluate to a bool",
                    ));
                }
            }
        }
        Ok(())
    }
}

fn eval_expr<'m>((expr, span): &'m SpannedExpr, env: &Environment<'m>) -> Result<Value, Diagnostic> {
    match expr.as_ref() {
        Expr::Num(n, unit) => Ok(Value::Num(*n, unit.clone().unwrap_or_default())),
        Expr::Bool(b) => Ok(Value::Bool(*b)),
        Expr::Neg(x) => match eval_expr(x, env)? {
            Value::Num(n, unit) => Ok(Value::Num(-n, unit)),
            _ => Err(Diagnostic::new(
                *span,
                "unary minus requires a numeric operand",
            )),
        },
        Expr::Bin(a, op, b) => {
            let x = eval_expr(a, env)?;
            let y = eval_expr(b, env)?;
            apply_binop(*op, x, y, *span)
        }
        Expr::If { cond, then, else_ } => match eval_expr(cond, env)? {
            Value::Bool(c) => {
                if c {
                    eval_expr(then, env)
                } else {
                    eval_expr(else_, env)
                }
            }
            _ => Err(Diagnostic::new(
                *span,
                "condition in `if` expression must be a bool",
            )),
        },
        Expr::Call(name, args) => {
            if name == "spread" {
                let value = eval_expr(&args[0], env)?;
                return eval_spread(value, env, *span);
            }
            let mut vals: Vec<(Decimal, Unit)> = Vec::with_capacity(args.len());
            for a in args {
                match eval_expr(a, env)? {
                    Value::Num(n, u) => vals.push((n, u)),
                    _ => {
                        return Err(Diagnostic::new(
                            a.1,
                            format!("argument to `{name}` must be numeric"),
                        ))
                    }
                }
            }
            if BUILTINS.iter().any(|(n, _)| *n == name.as_str()) {
                call_builtin(name, &vals, *span)
            } else if let Some(fn_def) = env.fns.get(name.as_str()).cloned() {
                let arg_values: Vec<Value> = vals.into_iter().map(|(n, u)| Value::Num(n, u)).collect();
                eval_fn_body(&fn_def, &arg_values, env, *span)
            } else {
                Err(Diagnostic::new(*span, format!("unknown function `{name}`")))
            }
        }
        Expr::Ref(path) => {
            // Bare leg name: resolves to the current-day value within the same flow (0 if not fired yet).
            if path.0.len() == 1 && let Some(entry) = env.current_entry {
                let key = (entry, path.0[0].as_str());
                if env.leg_set.contains(&key) {
                    return Ok(env.leg_values.get(&key).cloned()
                        .unwrap_or_else(|| Value::Num(Decimal::ZERO, Unit::scalar())));
                }
            }
            match resolve_ref(path, &env.stock_set, &env.param_set) {
                Some(RefKind::Stock(p)) => {
                    if let Some(&open_date) = env.opening_dates.get(&p) {
                        if env.current_date < open_date {
                            return Err(Diagnostic::new(
                                *span,
                                format!("account `{p}` opens on {open_date}, but referenced on {}", env.current_date),
                            ));
                        }
                    }
                    Ok(Value::Num(*env.stocks.get(&p).unwrap_or(&Decimal::ZERO), Unit::scalar()))
                }
                Some(RefKind::Param(n)) => {
                    env.params.get(&n).cloned().ok_or_else(|| {
                        Diagnostic::new(*span, format!("param `{n}` has no active interval"))
                    })
                }
                None => Err(Diagnostic::new(
                    *span,
                    format!("unknown reference `{path}`"),
                )),
            }
        }
        Expr::ParamAgg(flow_opt, leg, kind) => {
            let flow_key: &'m str = match flow_opt {
                Some(flow) => flow.as_str(),
                None => env.current_entry.ok_or_else(|| {
                    Diagnostic::new(*span, format!("aggregate `{leg}` used outside of an entry"))
                })?,
            };
            let key = (flow_key, leg.as_str());
            Ok(env.accumulators.get(&(key, *kind)).cloned()
                .unwrap_or_else(|| Value::Num(Decimal::ZERO, Unit::scalar())))
        }
    }
}

/// (name, arity)
pub const BUILTINS: &[(&str, usize)] = &[
    ("min", 2),
    ("max", 2),
    ("abs", 1),
    ("floor", 1),
    ("ceil", 1),
    ("round", 1),
    ("spread", 1),
];

fn call_builtin(name: &str, args: &[(Decimal, Unit)], span: Span) -> Result<Value, Diagnostic> {
    // Returns the appropriate result unit for min/max given two operands, applying
    // zero universality: a zero scalar is compatible with any unit.
    let pick_unit = |xu: &Unit, yu: &Unit| -> Result<Unit, String> {
        if xu.is_compatible_with(yu) {
            Ok(xu.clone())
        } else {
            Err(format!("cannot compare `{xu}` and `{yu}`: incompatible units"))
        }
    };
    match name {
        "min" => {
            let (x, xu) = &args[0];
            let (y, yu) = &args[1];
            let unit = pick_unit(xu, yu).map_err(|e| Diagnostic::new(span, e))?;
            Ok(Value::Num((*x).min(*y), unit))
        }
        "max" => {
            let (x, xu) = &args[0];
            let (y, yu) = &args[1];
            let unit = pick_unit(xu, yu).map_err(|e| Diagnostic::new(span, e))?;
            Ok(Value::Num((*x).max(*y), unit))
        }
        "abs"   => Ok(Value::Num(args[0].0.abs(),   args[0].1.clone())),
        "floor" => Ok(Value::Num(args[0].0.floor(), args[0].1.clone())),
        "ceil"  => Ok(Value::Num(args[0].0.ceil(),  args[0].1.clone())),
        "round" => Ok(Value::Num(args[0].0.round(), args[0].1.clone())),
        other => Err(Diagnostic::new(span, format!("unknown function `{other}`"))),
    }
}

fn check_account_open(env: &Environment<'_>, account: &Path, span: Span) -> Result<(), Diagnostic> {
    if let Some(&open_date) = env.opening_dates.get(account) {
        if env.current_date < open_date {
            return Err(Diagnostic::new(
                span,
                format!("account `{account}` opens on {open_date}, but referenced on {}", env.current_date),
            ));
        }
    }
    Ok(())
}

fn eval_fn_body(
    fn_def: &FnDef,
    arg_values: &[Value],
    env: &Environment<'_>,
    call_span: Span,
) -> Result<Value, Diagnostic> {
    let mut scope: HashMap<String, Value> = fn_def
        .params
        .iter()
        .zip(arg_values.iter())
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();

    for stmt in &fn_def.body {
        match stmt {
            Stmt::Let { name, value } => {
                let v = eval_fn_expr(value, &scope, env)?;
                scope.insert(name.clone(), v);
            }
            Stmt::Return(expr) => {
                return eval_fn_expr(expr, &scope, env);
            }
        }
    }
    Err(Diagnostic::new(call_span, "function has no return statement"))
}

fn eval_fn_expr(
    (expr, span): &SpannedExpr,
    scope: &HashMap<String, Value>,
    env: &Environment<'_>,
) -> Result<Value, Diagnostic> {
    match expr.as_ref() {
        Expr::Num(n, unit) => Ok(Value::Num(*n, unit.clone().unwrap_or_default())),
        Expr::Bool(b) => Ok(Value::Bool(*b)),
        Expr::Ref(path) => {
            if path.0.len() == 1 {
                if let Some(v) = scope.get(&path.0[0]) {
                    return Ok(v.clone());
                }
            }
            Err(Diagnostic::new(*span, format!("unknown local `{path}`")))
        }
        Expr::Neg(x) => match eval_fn_expr(x, scope, env)? {
            Value::Num(n, unit) => Ok(Value::Num(-n, unit)),
            _ => Err(Diagnostic::new(*span, "unary minus requires a numeric operand")),
        },
        Expr::Bin(a, op, b) => {
            let x = eval_fn_expr(a, scope, env)?;
            let y = eval_fn_expr(b, scope, env)?;
            apply_binop(*op, x, y, *span)
        }
        Expr::If { cond, then, else_ } => match eval_fn_expr(cond, scope, env)? {
            Value::Bool(c) => {
                if c { eval_fn_expr(then, scope, env) } else { eval_fn_expr(else_, scope, env) }
            }
            _ => Err(Diagnostic::new(*span, "condition in `if` expression must be a bool")),
        },
        Expr::Call(name, args) => {
            if name == "spread" {
                return Err(Diagnostic::new(*span, "spread() cannot be used inside a function body"));
            }
            let mut vals: Vec<(Decimal, Unit)> = Vec::with_capacity(args.len());
            for a in args {
                match eval_fn_expr(a, scope, env)? {
                    Value::Num(n, u) => vals.push((n, u)),
                    _ => return Err(Diagnostic::new(
                        a.1,
                        format!("argument to `{name}` must be numeric"),
                    )),
                }
            }
            if BUILTINS.iter().any(|(n, _)| *n == name.as_str()) {
                call_builtin(name, &vals, *span)
            } else if let Some(callee) = env.fns.get(name.as_str()).cloned() {
                let arg_values: Vec<Value> = vals.into_iter().map(|(n, u)| Value::Num(n, u)).collect();
                eval_fn_body(&callee, &arg_values, env, *span)
            } else {
                Err(Diagnostic::new(*span, format!("unknown function `{name}`")))
            }
        }
        Expr::ParamAgg(..) => Err(Diagnostic::new(
            *span,
            "aggregations are not allowed in function bodies",
        )),
    }
}

fn eval_num<'m>(expr: &'m SpannedExpr, env: &Environment<'m>) -> Result<Decimal, Diagnostic> {
    match eval_expr(expr, env)? {
        Value::Num(n, _) => Ok(n),
        Value::Bool(_) => Err(Diagnostic::new(
            expr.1,
            "expected a numeric value, got bool",
        )),
    }
}

fn apply_binop(op: BinOp, a: Value, b: Value, span: Span) -> Result<Value, Diagnostic> {
    match op {
        BinOp::Add | BinOp::Sub => {
            let (Value::Num(x, xu), Value::Num(y, yu)) = (a, b) else {
                return Err(Diagnostic::new(span, "arithmetic operations require numeric operands"));
            };
            // Zero with scalar unit is compatible with any unit — handles uninitialized
            // accumulators which start as Value::Num(0, scalar) before any firings.
            let result_unit = if x.is_zero() && xu.is_scalar() {
                yu.clone()
            } else if y.is_zero() && yu.is_scalar() {
                xu.clone()
            } else if xu.is_compatible_with(&yu) {
                xu.clone()
            } else {
                return Err(Diagnostic::new(
                    span,
                    format!("cannot add/subtract `{xu}` and `{yu}`: incompatible units"),
                ));
            };
            Ok(Value::Num(if op == BinOp::Add { x + y } else { x - y }, result_unit))
        }
        BinOp::Mul => {
            let (Value::Num(x, xu), Value::Num(y, yu)) = (a, b) else {
                return Err(Diagnostic::new(span, "arithmetic operations require numeric operands"));
            };
            Ok(Value::Num(x * y, xu * yu))
        }
        BinOp::Div => {
            let (Value::Num(x, xu), Value::Num(y, yu)) = (a, b) else {
                return Err(Diagnostic::new(span, "arithmetic operations require numeric operands"));
            };
            if y.is_zero() {
                return Err(Diagnostic::new(span, "division by zero"));
            }
            Ok(Value::Num(x / y, xu / yu))
        }
        BinOp::Lt | BinOp::LtEq | BinOp::Gt | BinOp::GtEq => {
            let (Value::Num(x, xu), Value::Num(y, yu)) = (a, b) else {
                return Err(Diagnostic::new(span, "comparison operations require numeric operands"));
            };
            if !xu.is_compatible_with(&yu) {
                return Err(Diagnostic::new(
                    span,
                    format!("cannot compare `{xu}` and `{yu}`: incompatible units"),
                ));
            }
            Ok(Value::Bool(match op {
                BinOp::Lt => x < y,
                BinOp::LtEq => x <= y,
                BinOp::Gt => x > y,
                BinOp::GtEq => x >= y,
                _ => unreachable!(),
            }))
        }
        BinOp::Eq | BinOp::NotEq => match (a, b) {
            (Value::Num(x, xu), Value::Num(y, yu)) => {
                if !xu.is_compatible_with(&yu) {
                    return Err(Diagnostic::new(
                        span,
                        format!("cannot compare `{xu}` and `{yu}`: incompatible units"),
                    ));
                }
                Ok(Value::Bool(if op == BinOp::Eq { x == y } else { x != y }))
            }
            (Value::Bool(x), Value::Bool(y)) => {
                Ok(Value::Bool(if op == BinOp::Eq { x == y } else { x != y }))
            }
            _ => Err(Diagnostic::new(span, "cannot compare values of different types")),
        },
    }
}

fn eval_spread(value: Value, env: &Environment<'_>, span: Span) -> Result<Value, Diagnostic> {
    let Value::Num(amount, unit) = value else {
        return Err(Diagnostic::new(span, "spread() requires a numeric argument"));
    };

    let schedule = env.current_entry_schedule.ok_or_else(|| {
        Diagnostic::new(span, "spread() can only be used inside an entry")
    })?;

    // The time dimension is the one with exponent -1 (the "per X" part).
    let time_dim = unit
        .dims()
        .find(|(_, exp)| *exp == -1)
        .map(|(dim, _)| dim.to_string())
        .ok_or_else(|| {
            Diagnostic::new(
                span,
                "spread() requires a value with a time dimension (e.g. usd/year)",
            )
        })?;

    let (period_start, period_end) = period_window(&time_dim, env.current_date).ok_or_else(|| {
        Diagnostic::new(
            span,
            format!("spread() does not recognise time dimension `{time_dim}`; expected year, month, or quarter"),
        )
    })?;

    let total = count_firings(schedule, period_start, period_end);
    let index = count_firings(schedule, period_start, env.current_date.succ_opt().unwrap_or(period_end));

    // Cancel the time dimension from the result unit.
    let result_unit = unit * Unit::single(time_dim);

    if total == 0 {
        return Ok(Value::Num(Decimal::ZERO, result_unit));
    }

    // Distribute amount evenly in cents using a Bresenham-style allocation so that the
    // extra cents are spread across the period rather than front-loaded.
    // Firing k (0-indexed) gets the extra cent when (k × remainder) % total < remainder.
    let total_cents = (amount * Decimal::ONE_HUNDRED).trunc();
    let total_cents_i64 = rust_decimal::prelude::ToPrimitive::to_i64(&total_cents).unwrap_or(0);
    let total_i64 = total as i64;
    let base = total_cents_i64 / total_i64;
    let remainder = total_cents_i64 % total_i64;
    let k = (index as i64) - 1;
    let gets_extra = remainder > 0 && (k * remainder) % total_i64 < remainder;
    let this_cents = if gets_extra { base + 1 } else { base };

    Ok(Value::Num(Decimal::new(this_cents, 2), result_unit))
}

/// Returns the [start, end) window for the calendar period named by `time_dim`.
fn period_window(time_dim: &str, date: NaiveDate) -> Option<(NaiveDate, NaiveDate)> {
    match time_dim {
        "year" => Some((
            NaiveDate::from_ymd_opt(date.year(), 1, 1)?,
            NaiveDate::from_ymd_opt(date.year() + 1, 1, 1)?,
        )),
        "month" => {
            let start = NaiveDate::from_ymd_opt(date.year(), date.month(), 1)?;
            let end = if date.month() == 12 {
                NaiveDate::from_ymd_opt(date.year() + 1, 1, 1)?
            } else {
                NaiveDate::from_ymd_opt(date.year(), date.month() + 1, 1)?
            };
            Some((start, end))
        }
        "quarter" => {
            let q_start_month = ((date.month() - 1) / 3) * 3 + 1;
            let end_month = q_start_month + 3;
            let (end_year, end_month) = if end_month > 12 {
                (date.year() + 1, end_month - 12)
            } else {
                (date.year(), end_month)
            };
            Some((
                NaiveDate::from_ymd_opt(date.year(), q_start_month, 1)?,
                NaiveDate::from_ymd_opt(end_year, end_month, 1)?,
            ))
        }
        _ => None,
    }
}

/// Count how many times `schedule` matches in `[from, to)`.
fn count_firings(schedule: &Schedule, from: NaiveDate, to: NaiveDate) -> usize {
    let mut count = 0;
    let mut d = from;
    while d < to {
        if schedule.matches(d) {
            count += 1;
        }
        d = match d.succ_opt() {
            Some(next) => next,
            None => break,
        };
    }
    count
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::Span;

    fn span() -> Span { Span::new(0, 0) }
    fn num(n: i64) -> Value { Value::Num(Decimal::new(n, 0), Unit::scalar()) }
    fn usd(n: i64) -> Value { Value::Num(Decimal::new(n, 0), Unit::single("usd")) }
    fn usd_per_year(n: i64) -> Value {
        Value::Num(Decimal::new(n, 0), Unit::parse("usd/year").unwrap())
    }

    #[test]
    fn add_same_unit() {
        let r = apply_binop(BinOp::Add, usd(100), usd(200), span()).unwrap();
        assert_eq!(r, usd(300));
    }

    #[test]
    fn add_incompatible_units_errors() {
        let sek = Value::Num(Decimal::new(100, 0), Unit::single("sek"));
        assert!(apply_binop(BinOp::Add, usd(100), sek, span()).is_err());
    }

    #[test]
    fn sub_same_unit() {
        let r = apply_binop(BinOp::Sub, usd(300), usd(100), span()).unwrap();
        assert_eq!(r, usd(200));
    }

    #[test]
    fn mul_produces_combined_unit() {
        let rate = Value::Num(Decimal::new(2, 0), Unit::parse("usd/year").unwrap());
        let years = Value::Num(Decimal::new(3, 0), Unit::single("year"));
        let r = apply_binop(BinOp::Mul, rate, years, span()).unwrap();
        assert_eq!(r, usd(6));
    }

    #[test]
    fn div_produces_combined_unit() {
        let r = apply_binop(BinOp::Div, usd_per_year(120_000), num(12), span()).unwrap();
        assert_eq!(r, usd_per_year(10_000));
    }

    #[test]
    fn div_cancels_unit() {
        let r = apply_binop(BinOp::Div, usd(300), usd(100), span()).unwrap();
        assert_eq!(r, num(3));
    }

    #[test]
    fn cmp_incompatible_units_errors() {
        let sek = Value::Num(Decimal::new(100, 0), Unit::single("sek"));
        assert!(apply_binop(BinOp::Lt, usd(100), sek, span()).is_err());
    }

    #[test]
    fn eq_same_unit() {
        assert_eq!(
            apply_binop(BinOp::Eq, usd(100), usd(100), span()).unwrap(),
            Value::Bool(true)
        );
    }

    #[test]
    fn eq_incompatible_units_errors() {
        let sek = Value::Num(Decimal::new(100, 0), Unit::single("sek"));
        assert!(apply_binop(BinOp::Eq, usd(100), sek, span()).is_err());
    }

    #[test]
    fn spread_distributes_annual_salary_monthly() {
        // 12_001 usd/year spread monthly = 12 months.
        // 12_001 / 12 = 1000.08333... → 1 month gets 1000.09, 11 get 1000.08? No:
        // total_cents = 1_200_100. base = 100008, remainder = 4. First 4 months → 1000.09.
        let src = r#"
            account Assets:Cash
            account Income:Salary
            param salary : usd/year = 12_001 usd/year
            entry monthly "paycheck" {
                Assets:Cash = spread(salary)
                Income:Salary
            }
        "#;
        let tokens = crate::lexer::lex(src).unwrap();
        let prog = crate::parser::parse(tokens).unwrap();
        let model = crate::resolver::resolve(&prog).unwrap();

        let start = chrono::NaiveDate::from_ymd_opt(2026, 1, 1).unwrap();
        let end = chrono::NaiveDate::from_ymd_opt(2026, 12, 31).unwrap();
        let log = model.simulate(start, end).unwrap();

        let amounts: Vec<Decimal> = log.transactions.iter()
            .filter(|t| t.label == "paycheck")
            .map(|t| t.postings.iter().find(|(p, _, _)| p.0.last().unwrap() == "Cash").unwrap().1)
            .collect();

        assert_eq!(amounts.len(), 12);
        let total: Decimal = amounts.iter().sum();
        assert_eq!(total, Decimal::new(12_001, 0), "total must be exact");

        // total_cents=1_200_100, total=12, base=100_008, remainder=4.
        // Extras spread via Bresenham: k=0,3,6,9 → months 1,4,7,10.
        let hi = Decimal::new(100009, 2);
        let lo = Decimal::new(100008, 2);
        // counts
        assert_eq!(amounts.iter().filter(|&&a| a == hi).count(), 4);
        assert_eq!(amounts.iter().filter(|&&a| a == lo).count(), 8);
        // positions: 0-indexed months 0,3,6,9 get the extra cent
        for (i, &amt) in amounts.iter().enumerate() {
            let k = i as i64;
            let expected = if (k * 4) % 12 < 4 { hi } else { lo };
            assert_eq!(amt, expected, "month {} has wrong amount", i + 1);
        }
    }

    #[test]
    fn param_unit_annotation_propagates_through_eval() {
        let src = r#"
            account Assets:Cash
            account Liabilities:Loan
            param salary_rate : usd/year = 120_000 usd/year
            entry monthly "paycheck" {
                Assets:Cash = salary_rate / 12
                Liabilities:Loan
            }
        "#;
        let tokens = crate::lexer::lex(src).unwrap();
        let prog = crate::parser::parse(tokens).unwrap();
        let model = crate::resolver::resolve(&prog).unwrap();

        let start = chrono::NaiveDate::from_ymd_opt(2026, 1, 1).unwrap();
        let end = chrono::NaiveDate::from_ymd_opt(2026, 1, 31).unwrap();
        let log = model.simulate(start, end).unwrap();

        // The monthly paycheck should be 120_000 / 12 = 10_000
        let tx = log.transactions.iter().find(|t| t.label == "paycheck").unwrap();
        let (_, amt, _) = tx.postings.iter().find(|(p, _, _)| p.0.last().unwrap() == "Cash").unwrap();
        assert_eq!(*amt, Decimal::new(10_000, 0));
    }
}
