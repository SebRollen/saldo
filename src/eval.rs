use crate::ast::{AggKind, BinOp, Expr, ParamBody, Path, PostingAmount, Span, SpannedExpr};
use crate::errors::Diagnostic;
use crate::resolve::{resolve_ref, Model, RefKind};
use chrono::{Datelike, Duration, NaiveDate};
use indexmap::IndexMap;
use rust_decimal::Decimal;
use std::collections::{HashMap, HashSet};

#[derive(Clone, Debug, PartialEq, PartialOrd)]
pub enum Value {
    Num(Decimal),
    Bool(bool),
}

pub struct SimLog {
    pub transactions: Vec<Transaction>,
    pub snapshots: Vec<DaySnapshot>,
}

pub struct Transaction {
    pub date: NaiveDate,
    pub flow: String,
    pub postings: Vec<(Path, Decimal)>,
}

pub struct DaySnapshot {
    pub date: NaiveDate,
    pub balances: IndexMap<Path, Decimal>,
}

struct Environment {
    stocks: HashMap<Path, Decimal>,
    params: HashMap<String, Decimal>,
    /// (flow_name, leg_name) → value for the current day (0 on non-firing days).
    leg_values: HashMap<(String, String), Decimal>,
    /// ((flow_name, leg_name), period) → running total.
    accumulators: HashMap<((String, String), AggKind), Decimal>,
    stock_set: HashSet<Path>,
    param_set: HashSet<String>,
    /// All declared (flow_name, leg_name) pairs.
    leg_set: HashSet<(String, String)>,
    /// Name of the flow currently being evaluated (empty outside of flows).
    current_flow: String,
}

impl Environment {
    fn new(model: &Model) -> Self {
        let stock_set = model.stocks.keys().cloned().collect();
        let param_set = model.params.keys().cloned().collect();
        let leg_set = model.leg_names.clone();
        Self {
            stocks: HashMap::new(),
            params: HashMap::new(),
            leg_values: HashMap::new(),
            accumulators: HashMap::new(),
            stock_set,
            param_set,
            leg_set,
            current_flow: String::new(),
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
}

impl Model {
    pub fn simulate(&self, start: NaiveDate, end: NaiveDate) -> Result<SimLog, Diagnostic> {
        let mut env = Environment::new(self);
        let mut log = SimLog {
            transactions: Vec::new(),
            snapshots: Vec::new(),
        };

        self.initialize_stocks(&mut env)?;

        let mut t = start;
        while t <= end {
            env.leg_values.clear();
            reset_periods(t, &mut env);
            self.evaluate_params(t, &mut env)?;
            let txs = self.apply_flows(t, &mut env)?;
            log.transactions.extend(txs);
            self.check_assertions(t, &env)?;

            // Snapshot current balances.
            let balances: IndexMap<Path, Decimal> = self
                .stocks
                .keys()
                .map(|p| (p.clone(), *env.stocks.get(p).unwrap_or(&Decimal::ZERO)))
                .collect();
            log.snapshots.push(DaySnapshot { date: t, balances });

            advance_period(&mut env);

            t = t
                .checked_add_signed(Duration::days(1))
                .ok_or_else(|| Diagnostic::new((0..0).into(), "date overflow"))?;
        }

        Ok(log)
    }

    fn initialize_stocks(&self, env: &mut Environment) -> Result<(), Diagnostic> {
        for (name, account) in &self.stocks {
            let v = match &account.init {
                Some(e) => eval_num(e, env)?,
                None => Decimal::ZERO,
            };
            env.add_stock(name.clone(), v);
        }
        Ok(())
    }

    fn evaluate_params(&self, t: NaiveDate, env: &mut Environment) -> Result<(), Diagnostic> {
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

    fn apply_flows(
        &self,
        t: NaiveDate,
        env: &mut Environment,
    ) -> Result<Vec<Transaction>, Diagnostic> {
        let mut txs = Vec::new();
        for flow in &self.flows {
            if !flow.schedule.matches(t) {
                continue;
            }
            env.current_flow = flow.key().to_string();
            let mut explicit: Vec<(Path, Decimal)> = Vec::new();
            let mut auto_leg: Option<(Path, Option<String>)> = None;

            for posting in &flow.postings {
                match &posting.amount {
                    Some(PostingAmount::Expr(e)) => {
                        let amt = eval_num(e, env).map_err(|d| {
                            d.with_note(flow.span, format!("in flow `{}`", flow.label))
                        })?;
                        let amt = amt.round_dp(2);
                        if let Some(leg) = &posting.leg_name {
                            env.leg_values
                                .insert((flow.key().to_string(), leg.clone()), amt);
                        }
                        explicit.push((posting.account.clone(), amt));
                    }
                    Some(PostingAmount::All) => {
                        // Clear the current balance: amt = -(current balance).
                        let current = *env.stocks.get(&posting.account).unwrap_or(&Decimal::ZERO);
                        let amt = -current.round_dp(2);
                        if let Some(leg) = &posting.leg_name {
                            env.leg_values
                                .insert((flow.key().to_string(), leg.clone()), amt);
                        }
                        explicit.push((posting.account.clone(), amt));
                    }
                    None => {
                        auto_leg = Some((posting.account.clone(), posting.leg_name.clone()));
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
                    env.leg_values.insert((flow.key().to_string(), leg), auto);
                }
                *env.stocks_mut()
                    .entry(account.clone())
                    .or_insert(Decimal::ZERO) += auto;
                postings.push((account, auto));
            }

            txs.push(Transaction {
                date: t,
                flow: flow.label.clone(),
                postings,
            });
        }
        env.current_flow = String::new();
        Ok(txs)
    }

    fn check_assertions(&self, t: NaiveDate, env: &Environment) -> Result<(), Diagnostic> {
        for (sched, expr) in &self.asserts {
            if !sched.matches(t) {
                continue;
            }
            let span = expr.1;
            match eval_expr(expr, env)? {
                Value::Bool(true) => {}
                Value::Bool(false) => {
                    return Err(Diagnostic::new(span, format!("assertion failed on {t}")));
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

fn reset_periods(t: NaiveDate, env: &mut Environment) {
    if t.day() == 1 {
        env.accumulators.retain(|(_, k), _| *k != AggKind::Mtd);
    }
    if t.day() == 1 && matches!(t.month(), 1 | 4 | 7 | 10) {
        env.accumulators.retain(|(_, k), _| *k != AggKind::Qtd);
    }
    if t.month() == 1 && t.day() == 1 {
        env.accumulators.retain(|(_, k), _| *k != AggKind::Ytd);
    }
}

fn advance_period(env: &mut Environment) {
    // Accumulate named leg values into period totals. Runs every day; since leg_values
    // is cleared at the start of each day and only populated when a flow fires, this
    // is a no-op on non-firing days and accumulates the actual amount on firing days.
    for (key, &value) in &env.leg_values {
        for kind in [AggKind::Mtd, AggKind::Qtd, AggKind::Ytd] {
            *env.accumulators
                .entry((key.clone(), kind))
                .or_insert(Decimal::ZERO) += value;
        }
    }
}

fn eval_expr((expr, span): &SpannedExpr, env: &Environment) -> Result<Value, Diagnostic> {
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
            call_builtin(name, &nums, *span)
        }
        Expr::Ref(path) => {
            // Bare leg name: resolves to the current-day value within the same flow (0 if not fired yet).
            if path.0.len() == 1 && !env.current_flow.is_empty() {
                let key = (env.current_flow.clone(), path.0[0].clone());
                if env.leg_set.contains(&key) {
                    return Ok(Value::Num(
                        env.leg_values.get(&key).copied().unwrap_or(Decimal::ZERO),
                    ));
                }
            }
            match resolve_ref(path, &env.stock_set, &env.param_set) {
                Some(RefKind::Stock(p)) => {
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
            let key = match flow_opt {
                Some(flow) => (flow.clone(), leg.clone()),
                None => (env.current_flow.clone(), leg.clone()),
            };
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

fn eval_num(expr: &SpannedExpr, env: &Environment) -> Result<Decimal, Diagnostic> {
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
            Ok(Value::Num(match op {
                BinOp::Add => x + y,
                BinOp::Sub => x - y,
                BinOp::Mul => x * y,
                BinOp::Div => x / y,
                _ => unreachable!(),
            }))
        }
        BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge => {
            let (Value::Num(x), Value::Num(y)) = (a, b) else {
                return Err(Diagnostic::new(
                    span,
                    "comparison operations require numeric operands",
                ));
            };
            Ok(Value::Bool(match op {
                BinOp::Lt => x < y,
                BinOp::Le => x <= y,
                BinOp::Gt => x > y,
                BinOp::Ge => x >= y,
                _ => unreachable!(),
            }))
        }
        BinOp::Eq => Ok(Value::Bool(a == b)),
    }
}
