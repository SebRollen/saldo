use crate::ast::{AggKind, BinOp, Expr, ParamBody, Path, PostingAmount, Span, SpannedExpr, Stmt};
use crate::errors::Diagnostic;
use crate::resolver::{resolve_ref, FnDef, Model, RefKind};
use chrono::{Datelike, Duration, NaiveDate};
use indexmap::IndexMap;
use rust_decimal::Decimal;
use std::collections::{HashMap, HashSet};
use std::fmt;

#[derive(Clone, Debug, PartialEq, PartialOrd)]
pub enum Value {
    Num(Decimal),
    Bool(bool),
}

impl fmt::Display for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Value::Num(n) => write!(f, "{n}"),
            Value::Bool(b) => write!(f, "{b}"),
        }
    }
}

#[derive(Debug)]
pub struct SimLog {
    pub transactions: Vec<Transaction>,
    pub snapshots: Vec<DaySnapshot>,
    pub opening: IndexMap<Path, Decimal>,
}

#[derive(Debug)]
pub struct Transaction {
    pub date: NaiveDate,
    pub label: String,
    pub postings: Vec<(Path, Decimal)>,
}

#[derive(Debug)]
pub struct DaySnapshot {
    pub date: NaiveDate,
    pub balances: IndexMap<Path, Decimal>,
}

struct Environment<'m> {
    stocks: HashMap<Path, Decimal>,
    params: HashMap<String, Decimal>,
    /// (flow_name, leg_name) → value for the current day (0 on non-firing days).
    leg_values: HashMap<(&'m str, &'m str), Decimal>,
    /// ((flow_name, leg_name), period) → running total.
    accumulators: HashMap<((&'m str, &'m str), AggKind), Decimal>,
    stock_set: HashSet<Path>,
    param_set: HashSet<String>,
    /// All declared (flow_name, leg_name) pairs.
    leg_set: HashSet<(&'m str, &'m str)>,
    /// Name of the flow currently being evaluated
    current_entry: Option<&'m str>,
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
        let fns = model.fns.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
        Self {
            stocks: HashMap::new(),
            params: HashMap::new(),
            leg_values: HashMap::new(),
            accumulators: HashMap::new(),
            stock_set,
            param_set,
            leg_set,
            current_entry: None,
            opening_dates,
            current_date: NaiveDate::MIN,
            fns,
        }
    }

    fn add_stock(&mut self, name: Path, value: Decimal) {
        self.stocks.insert(name, value);
    }

    fn add_param(&mut self, name: String, value: Decimal) {
        self.params.insert(name, value);
    }

    fn stocks_mut(&mut self) -> &mut HashMap<Path, Decimal> {
        &mut self.stocks
    }

    fn advance_period(&mut self) {
        // Accumulate named leg values into period totals. Runs every day; since leg_values
        // is cleared at the start of each day and only populated when a flow fires, this
        // is a no-op on non-firing days and accumulates the actual amount on firing days.
        for (key, &value) in &self.leg_values {
            for kind in [AggKind::Mtd, AggKind::Qtd, AggKind::Ytd] {
                *self
                    .accumulators
                    .entry((*key, kind))
                    .or_insert(Decimal::ZERO) += value;
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
        for (name, body) in &self.params {
            match body {
                ParamBody::Const(e) => {
                    let v = eval_num(e, env)?;
                    env.add_param(name.clone(), v);
                }
                ParamBody::Schedule(intervals) => {
                    if let Some(iv) = intervals.iter().find(|iv| iv.contains(t)) {
                        let v = eval_num(&iv.value, env)?;
                        env.add_param(name.clone(), v);
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
            let mut explicit: Vec<(Path, Decimal)> = Vec::new();
            let mut auto_leg: Option<(Path, Option<&'m str>)> = None;

            for posting in &entry.postings {
                check_account_open(env, &posting.account, entry.span)?;
                match &posting.amount {
                    Some(PostingAmount::Expr(e)) => {
                        let amt = eval_num(e, env).map_err(|d| {
                            d.with_note(entry.span, format!("in entry `{}`", entry.label))
                        })?;
                        let amt = amt.round_dp(2);
                        if let Some(leg) = &posting.leg_name {
                            env.leg_values
                                .insert((entry.key.as_str(), leg.as_str()), amt);
                        }
                        explicit.push((posting.account.clone(), amt));
                    }
                    Some(PostingAmount::All) => {
                        // Clear the current balance: amt = -(current balance).
                        let current = *env.stocks.get(&posting.account).unwrap_or(&Decimal::ZERO);
                        let amt = -current.round_dp(2);
                        if let Some(leg) = &posting.leg_name {
                            env.leg_values
                                .insert((entry.key.as_str(), leg.as_str()), amt);
                        }
                        explicit.push((posting.account.clone(), amt));
                    }
                    None => {
                        auto_leg = Some((posting.account.clone(), posting.leg_name.as_deref()));
                    }
                }
            }

            let explicit_sum: Decimal = explicit.iter().map(|(_, c)| c).sum();

            // Apply explicit postings to balances.
            for (account, amt) in &explicit {
                *env.stocks_mut()
                    .entry(account.clone())
                    .or_insert(Decimal::ZERO) += amt;
            }

            // Auto-balance posting: exact negation guarantees the transaction sums to zero.
            let mut postings = explicit;
            if let Some((account, leg_name)) = auto_leg {
                let auto = -explicit_sum;
                if let Some(leg) = leg_name {
                    env.leg_values.insert((entry.key.as_str(), leg), auto);
                }
                *env.stocks_mut()
                    .entry(account.clone())
                    .or_insert(Decimal::ZERO) += auto;
                postings.push((account, auto));
            }

            txs.push(Transaction {
                date: t,
                label: entry.label.clone(),
                postings,
            });
        }
        env.current_entry = None;
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
                Value::Num(_) => {
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
        Expr::Num(n) => Ok(Value::Num(*n)),
        Expr::Bool(b) => Ok(Value::Bool(*b)),
        Expr::Neg(x) => match eval_expr(x, env)? {
            Value::Num(n) => Ok(Value::Num(-n)),
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
            let mut nums = Vec::with_capacity(args.len());
            for a in args {
                match eval_expr(a, env)? {
                    Value::Num(n) => nums.push(n),
                    _ => {
                        return Err(Diagnostic::new(
                            a.1,
                            format!("argument to `{name}` must be numeric"),
                        ))
                    }
                }
            }
            if BUILTINS.iter().any(|(n, _)| *n == name.as_str()) {
                call_builtin(name, &nums, *span)
            } else if let Some(fn_def) = env.fns.get(name.as_str()).cloned() {
                eval_fn_body(&fn_def, &nums, env, *span)
            } else {
                Err(Diagnostic::new(*span, format!("unknown function `{name}`")))
            }
        }
        Expr::Ref(path) => {
            // Bare leg name: resolves to the current-day value within the same flow (0 if not fired yet).
            if path.0.len() == 1 && let Some(entry) = env.current_entry {
                let key = (entry, path.0[0].as_str());
                if env.leg_set.contains(&key) {
                    return Ok(Value::Num(
                        env.leg_values.get(&key).copied().unwrap_or(Decimal::ZERO),
                    ));
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
                    Ok(Value::Num(*env.stocks.get(&p).unwrap_or(&Decimal::ZERO)))
                }
                Some(RefKind::Param(n)) => {
                    Ok(Value::Num(*env.params.get(&n).ok_or_else(|| {
                        Diagnostic::new(*span, format!("param `{n}` has no active interval"))
                    })?))
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
            let v = env
                .accumulators
                .get(&(key, *kind))
                .copied()
                .unwrap_or(Decimal::ZERO);
            Ok(Value::Num(v))
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
];

fn call_builtin(name: &str, args: &[Decimal], span: Span) -> Result<Value, Diagnostic> {
    match name {
        "min" => Ok(Value::Num(args[0].min(args[1]))),
        "max" => Ok(Value::Num(args[0].max(args[1]))),
        "abs" => Ok(Value::Num(args[0].abs())),
        "floor" => Ok(Value::Num(args[0].floor())),
        "ceil" => Ok(Value::Num(args[0].ceil())),
        "round" => Ok(Value::Num(args[0].round())),
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
    arg_values: &[Decimal],
    env: &Environment<'_>,
    call_span: Span,
) -> Result<Value, Diagnostic> {
    let mut scope: HashMap<String, Decimal> = fn_def
        .params
        .iter()
        .zip(arg_values.iter())
        .map(|(k, &v)| (k.clone(), v))
        .collect();

    for stmt in &fn_def.body {
        match stmt {
            Stmt::Let { name, value } => {
                let v = eval_fn_expr(value, &scope, env)?;
                match v {
                    Value::Num(n) => { scope.insert(name.clone(), n); }
                    Value::Bool(_) => return Err(Diagnostic::new(
                        value.1,
                        format!("let binding `{name}` must evaluate to a number"),
                    )),
                }
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
    scope: &HashMap<String, Decimal>,
    env: &Environment<'_>,
) -> Result<Value, Diagnostic> {
    match expr.as_ref() {
        Expr::Num(n) => Ok(Value::Num(*n)),
        Expr::Bool(b) => Ok(Value::Bool(*b)),
        Expr::Ref(path) => {
            if path.0.len() == 1 {
                if let Some(&v) = scope.get(&path.0[0]) {
                    return Ok(Value::Num(v));
                }
            }
            Err(Diagnostic::new(*span, format!("unknown local `{path}`")))
        }
        Expr::Neg(x) => match eval_fn_expr(x, scope, env)? {
            Value::Num(n) => Ok(Value::Num(-n)),
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
            let mut nums = Vec::with_capacity(args.len());
            for a in args {
                match eval_fn_expr(a, scope, env)? {
                    Value::Num(n) => nums.push(n),
                    _ => return Err(Diagnostic::new(
                        a.1,
                        format!("argument to `{name}` must be numeric"),
                    )),
                }
            }
            if BUILTINS.iter().any(|(n, _)| *n == name.as_str()) {
                call_builtin(name, &nums, *span)
            } else if let Some(callee) = env.fns.get(name.as_str()).cloned() {
                eval_fn_body(&callee, &nums, env, *span)
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
        Value::Num(n) => Ok(n),
        Value::Bool(_) => Err(Diagnostic::new(
            expr.1,
            "expected a numeric value, got bool",
        )),
    }
}

fn apply_binop(op: BinOp, a: Value, b: Value, span: Span) -> Result<Value, Diagnostic> {
    match op {
        BinOp::Add | BinOp::Sub | BinOp::Mul | BinOp::Div => {
            let (Value::Num(x), Value::Num(y)) = (a, b) else {
                return Err(Diagnostic::new(
                    span,
                    "arithmetic operations require numeric operands",
                ));
            };
            if op == BinOp::Div && y.is_zero() {
                return Err(Diagnostic::new(span, "division by zero"));
            }
            Ok(Value::Num(match op {
                BinOp::Add => x + y,
                BinOp::Sub => x - y,
                BinOp::Mul => x * y,
                BinOp::Div => x / y,
                _ => unreachable!(),
            }))
        }
        BinOp::Lt | BinOp::LtEq | BinOp::Gt | BinOp::GtEq => {
            let (Value::Num(x), Value::Num(y)) = (a, b) else {
                return Err(Diagnostic::new(
                    span,
                    "comparison operations require numeric operands",
                ));
            };
            Ok(Value::Bool(match op {
                BinOp::Lt => x < y,
                BinOp::LtEq => x <= y,
                BinOp::Gt => x > y,
                BinOp::GtEq => x >= y,
                _ => unreachable!(),
            }))
        }
        BinOp::Eq => Ok(Value::Bool(a == b)),
        BinOp::NotEq => Ok(Value::Bool(a != b))
    }
}
