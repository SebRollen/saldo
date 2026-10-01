use crate::ast::{AggKind, BinOp, Expr, ParamBody, Path, PostingAmount, Span, SpannedExpr, Stmt};
use crate::errors::Diagnostic;
use crate::resolver::{FnDef, Model, RefKind, resolve_ref, walk_expr};
use chrono::{Datelike, Duration, NaiveDate};
use indexmap::IndexMap;
use rust_decimal::Decimal;
use std::collections::{HashMap, HashSet};
use std::fmt;

#[derive(Clone, Debug, PartialEq)]
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

/// A param's value for the current day.
enum ParamValue {
    Value(Decimal),
    /// A time-varying param with no interval covering the current day.
    Inactive,
    /// Evaluation failed; reported only if the param is read.
    Error(Diagnostic),
}

struct Environment<'m> {
    stocks: HashMap<Path, Decimal>,
    params: HashMap<String, ParamValue>,
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
    /// User-defined functions.
    fns: &'m IndexMap<String, FnDef>,
}

impl<'m> Environment<'m> {
    fn new(model: &'m Model) -> Self {
        let stock_set = model.stocks.keys().cloned().collect();
        let param_set = model.params.keys().cloned().collect();
        let leg_set = model
            .leg_names
            .iter()
            .map(|(a, b)| (a.as_str(), b.as_str()))
            .collect();
        let opening_dates = model
            .stocks
            .iter()
            .filter_map(|(p, a)| a.opening.as_ref().map(|(_, d)| (p.clone(), *d)))
            .collect();
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
            fns: &model.fns,
        }
    }

    fn add_stock(&mut self, name: Path, value: Decimal) {
        self.stocks.insert(name, value);
    }

    fn add_param(&mut self, name: String, value: ParamValue) {
        self.params.insert(name, value);
    }

    /// Adds `amount` to `account`'s balance.
    fn post(&mut self, account: &Path, amount: Decimal, span: Span) -> Result<(), Diagnostic> {
        let balance = self.stocks.entry(account.clone()).or_insert(Decimal::ZERO);
        *balance = balance
            .checked_add(amount)
            .ok_or_else(|| Diagnostic::new(span, format!("balance of `{account}` overflowed")))?;
        Ok(())
    }

    /// Accumulates named leg values into period totals. On overflow, returns the
    /// (entry key, leg name) whose total overflowed.
    fn advance_period(&mut self) -> Result<(), (&'m str, &'m str)> {
        // Runs every day; since leg_values is cleared at the start of each day and only
        // populated when a flow fires, this is a no-op on non-firing days and accumulates
        // the actual amount on firing days.
        for (key, &value) in &self.leg_values {
            for kind in [AggKind::Mtd, AggKind::Qtd, AggKind::Ytd] {
                let total = self
                    .accumulators
                    .entry((*key, kind))
                    .or_insert(Decimal::ZERO);
                *total = total.checked_add(value).ok_or(*key)?;
            }
        }
        Ok(())
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

        let effective_start = self.first_day(start);

        let mut log = SimLog {
            transactions: Vec::new(),
            snapshots: Vec::new(),
            opening: IndexMap::new(),
        };

        let mut t = effective_start;
        let mut opening_captured = false;
        while t <= end {
            env.current_date = t;
            env.leg_values.clear();
            env.reset_periods(t);
            self.evaluate_params(t, &mut env);

            // Initialize accounts whose opening date is today. Params may read the
            // new balances, so re-evaluate them if anything opened.
            let opened = self.open_accounts(t, &mut env)?;
            if !opened.is_empty() {
                self.evaluate_params(t, &mut env);
            }

            // Capture opening balances at the start of the user's simulation range,
            // after any accounts that open today are initialized but before entries fire.
            if t == start && !opening_captured {
                log.opening = self
                    .stocks
                    .keys()
                    .map(|p| (p.clone(), *env.stocks.get(p).unwrap_or(&Decimal::ZERO)))
                    .collect();
                if checked_sum(log.opening.values()).is_none() {
                    return Err(Diagnostic::new(
                        Span::new(0, 0),
                        format!("opening balances on {t} overflow when summed"),
                    ));
                }
                opening_captured = true;
            }

            // Accounts opening after the start of the range get their own
            // opening transaction so ledger output agrees with the balances.
            let mut txs = Vec::new();
            if t > start {
                txs.extend(opening_transaction(t, opened)?);
            }
            txs.extend(self.apply_flows(t, &mut env)?);
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

            if let Err((key, leg)) = env.advance_period() {
                let span = self
                    .entries
                    .iter()
                    .find(|e| e.key == key)
                    .map_or(Span::new(0, 0), |e| e.span);
                return Err(Diagnostic::new(
                    span,
                    format!("running total of leg `{leg}` overflowed"),
                ));
            }

            t = t
                .checked_add_signed(Duration::days(1))
                .ok_or_else(|| Diagnostic::new((0..0).into(), "date overflow"))?;
        }

        if !opening_captured {
            log.opening = self
                .stocks
                .keys()
                .map(|p| (p.clone(), *env.stocks.get(p).unwrap_or(&Decimal::ZERO)))
                .collect();
        }

        Ok(log)
    }

    /// The first simulated day when reporting from `start`: the earliest
    /// opening date if that's before `start`, so the warm-up period accumulates
    /// the right balances before reporting starts.
    pub fn first_day(&self, start: NaiveDate) -> NaiveDate {
        self.stocks
            .values()
            .filter_map(|a| a.opening.as_ref().map(|(_, d)| *d))
            .min()
            .map_or(start, |earliest| earliest.min(start))
    }

    /// Warns about `.ytd`/`.qtd`/`.mtd` totals that are missing amounts because
    /// the simulation starts partway through their period, after their entry
    /// would already have fired.
    pub fn missed_aggregate_warnings(&self, first_day: NaiveDate) -> Vec<Diagnostic> {
        struct Aggregate {
            entry_key: String,
            leg: String,
            kind: AggKind,
            /// How it's written, e.g. `paycheck.k401.ytd`.
            display: String,
            span: Span,
        }
        let mut aggregates: Vec<Aggregate> = Vec::new();
        let mut collect = |e: &SpannedExpr, entry_key: Option<&str>| {
            walk_expr(e, &mut |sub| {
                if let Expr::ParamAgg(qualifier, leg, kind) = sub.0.as_ref()
                    && let Some(key) = qualifier.as_deref().or(entry_key)
                {
                    let display = match qualifier {
                        Some(q) => format!("{q}.{leg}.{kind}"),
                        None => format!("{leg}.{kind}"),
                    };
                    aggregates.push(Aggregate {
                        entry_key: key.to_string(),
                        leg: leg.clone(),
                        kind: *kind,
                        display,
                        span: sub.1,
                    });
                }
            });
        };
        for body in self.params.values() {
            match body {
                ParamBody::Const(e) => collect(e, None),
                ParamBody::Schedule(intervals) => {
                    for iv in intervals {
                        collect(&iv.value, None);
                    }
                }
            }
        }
        for entry in &self.entries {
            for posting in &entry.postings {
                if let Some(PostingAmount::Expr(e)) = &posting.amount {
                    collect(e, Some(&entry.key));
                }
            }
        }
        for (_, e) in &self.asserts {
            collect(e, None);
        }
        aggregates.sort_by_key(|a| a.span.start);

        let mut warned = HashSet::new();
        let mut warnings = Vec::new();
        for agg in aggregates {
            if !warned.insert((agg.entry_key.clone(), agg.leg.clone(), agg.kind)) {
                continue;
            }
            let Some(entry) = self.entries.iter().find(|e| e.key == agg.entry_key) else {
                continue;
            };
            let period_start = agg.kind.period_start(first_day);
            let missed = period_start
                .iter_days()
                .take_while(|d| *d < first_day)
                .find(|d| entry.schedule.matches(*d));
            if let Some(missed) = missed {
                warnings.push(
                    Diagnostic::new(
                        agg.span,
                        format!(
                            "`{}` is missing amounts from before {first_day}",
                            agg.display
                        ),
                    )
                    .with_note(
                        agg.span,
                        format!(
                            "`{}` would have fired on {missed}; simulate from {period_start} \
                             (or open an account by then) to include it",
                            entry.label
                        ),
                    ),
                );
            }
        }
        warnings
    }

    /// Evaluates every param for day `t`. Failures are recorded rather than
    /// returned, so they only abort the simulation if the param is actually read.
    fn evaluate_params<'m>(&'m self, t: NaiveDate, env: &mut Environment<'m>) {
        for (name, body) in &self.params {
            let expr = match body {
                ParamBody::Const(e) => Some(e),
                ParamBody::Schedule(intervals) => intervals
                    .iter()
                    .find(|iv| iv.contains(t))
                    .map(|iv| &iv.value),
            };
            let value = match expr {
                Some(e) => match eval_num(e, env) {
                    Ok(v) => ParamValue::Value(v),
                    Err(d) => ParamValue::Error(d),
                },
                None => ParamValue::Inactive,
            };
            env.add_param(name.clone(), value);
        }
    }

    /// Initializes accounts whose opening date is `t`, in declaration order, and
    /// returns their opening balances.
    fn open_accounts<'m>(
        &'m self,
        t: NaiveDate,
        env: &mut Environment<'m>,
    ) -> Result<Vec<(Path, Decimal)>, Diagnostic> {
        let mut opened = Vec::new();
        for (name, account) in &self.stocks {
            if let Some((expr, date)) = &account.opening
                && *date == t
            {
                let v = to_amount(eval_num(expr, env)?);
                env.add_stock(name.clone(), v);
                opened.push((name.clone(), v));
            }
        }
        Ok(opened)
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
                check_account_open(env, &posting.account, posting.account_span)?;
                match &posting.amount {
                    Some(PostingAmount::Expr(e)) => {
                        let amt = eval_num(e, env).map_err(|d| {
                            d.with_note(entry.span, format!("in entry `{}`", entry.label))
                        })?;
                        let amt = to_amount(amt);
                        if let Some(leg) = &posting.leg_name {
                            env.leg_values
                                .insert((entry.key.as_str(), leg.as_str()), amt);
                        }
                        explicit.push((posting.account.clone(), amt));
                    }
                    Some(PostingAmount::All) => {
                        // Clear the current balance: amt = -(current balance).
                        let current = *env.stocks.get(&posting.account).unwrap_or(&Decimal::ZERO);
                        let amt = to_amount(-current);
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

            let explicit_sum = checked_sum(explicit.iter().map(|(_, c)| c)).ok_or_else(|| {
                Diagnostic::new(
                    entry.span,
                    format!("postings of entry `{}` overflow when summed", entry.label),
                )
            })?;
            if auto_leg.is_none() && !explicit_sum.is_zero() {
                return Err(Diagnostic::new(
                    entry.span,
                    format!(
                        "entry `{}` does not balance on {t}: postings sum to {explicit_sum}",
                        entry.label
                    ),
                ));
            }

            // Apply explicit postings to balances.
            for (account, amt) in &explicit {
                env.post(account, *amt, entry.span)?;
            }

            // Auto-balance posting: exact negation guarantees the transaction sums to zero.
            let mut postings = explicit;
            if let Some((account, leg_name)) = auto_leg {
                let auto = to_amount(-explicit_sum);
                if let Some(leg) = leg_name {
                    env.leg_values.insert((entry.key.as_str(), leg), auto);
                }
                env.post(&account, auto, entry.span)?;
                postings.push((account, auto));
            }

            // An entry that moves no money (e.g. `= all` on an empty account) is
            // not worth a ledger transaction.
            if postings.iter().any(|(_, amt)| !amt.is_zero()) {
                txs.push(Transaction {
                    date: t,
                    label: entry.label.clone(),
                    postings,
                });
            }
        }
        env.current_entry = None;
        Ok(txs)
    }

    fn check_assertions<'m>(
        &'m self,
        t: NaiveDate,
        env: &Environment<'m>,
    ) -> Result<(), Diagnostic> {
        for (sched, expr) in &self.asserts {
            if !sched.matches(t) {
                continue;
            }
            let span = expr.1;
            match eval_expr(expr, env, None)? {
                Value::Bool(true) => {}
                Value::Bool(false) => {
                    let msg = if let Expr::Bin(lhs, op, rhs) = expr.0.as_ref() {
                        if let (Ok(lv), Ok(rv)) =
                            (eval_expr(lhs, env, None), eval_expr(rhs, env, None))
                        {
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

/// Parameters and `let` bindings of the function being evaluated.
type Locals<'m> = HashMap<&'m str, Decimal>;

/// Evaluates `expr`. Inside a function body `locals` holds its bindings, and
/// names resolve only against them; at the top level it is `None`.
fn eval_expr<'m>(
    (expr, span): &'m SpannedExpr,
    env: &Environment<'m>,
    locals: Option<&Locals<'m>>,
) -> Result<Value, Diagnostic> {
    match expr.as_ref() {
        Expr::Num(n) => Ok(Value::Num(*n)),
        Expr::Bool(b) => Ok(Value::Bool(*b)),
        Expr::Neg(x) => match eval_expr(x, env, locals)? {
            Value::Num(n) => Ok(Value::Num(-n)),
            _ => Err(Diagnostic::new(
                *span,
                "unary minus requires a numeric operand",
            )),
        },
        Expr::Not(x) => match eval_expr(x, env, locals)? {
            Value::Bool(b) => Ok(Value::Bool(!b)),
            _ => Err(Diagnostic::new(*span, "`not` requires a bool operand")),
        },
        Expr::Bin(a, op @ (BinOp::And | BinOp::Or), b) => {
            let as_bool = |operand: &SpannedExpr, v| match v {
                Value::Bool(b) => Ok(b),
                Value::Num(_) => Err(Diagnostic::new(
                    operand.1,
                    format!("operands of `{op}` must be bools"),
                )),
            };
            let x = as_bool(a, eval_expr(a, env, locals)?)?;
            // Short-circuit: the right side is only evaluated when needed.
            if (*op == BinOp::And && !x) || (*op == BinOp::Or && x) {
                return Ok(Value::Bool(x));
            }
            Ok(Value::Bool(as_bool(b, eval_expr(b, env, locals)?)?))
        }
        Expr::Bin(a, op, b) => {
            let x = eval_expr(a, env, locals)?;
            let y = eval_expr(b, env, locals)?;
            apply_binop(*op, x, y, *span)
        }
        Expr::If { cond, then, else_ } => match eval_expr(cond, env, locals)? {
            Value::Bool(c) => {
                if c {
                    eval_expr(then, env, locals)
                } else {
                    eval_expr(else_, env, locals)
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
                match eval_expr(a, env, locals)? {
                    Value::Num(n) => nums.push(n),
                    _ => {
                        return Err(Diagnostic::new(
                            a.1,
                            format!("argument to `{name}` must be numeric"),
                        ));
                    }
                }
            }
            if BUILTINS.iter().any(|(n, _)| *n == name.as_str()) {
                call_builtin(name, &nums, *span)
            } else if let Some(fn_def) = env.fns.get(name.as_str()) {
                eval_fn_body(fn_def, &nums, env, *span)
            } else {
                Err(Diagnostic::new(*span, format!("unknown function `{name}`")))
            }
        }
        Expr::Ref(path) => {
            if let Some(locals) = locals {
                return match locals.get(path.0[0].as_str()) {
                    Some(&v) if path.0.len() == 1 => Ok(Value::Num(v)),
                    _ => Err(Diagnostic::new(*span, format!("unknown local `{path}`"))),
                };
            }
            // Bare leg name: resolves to the current-day value within the same flow (0 if not fired yet).
            if path.0.len() == 1
                && let Some(entry) = env.current_entry
            {
                let key = (entry, path.0[0].as_str());
                if env.leg_set.contains(&key) {
                    return Ok(Value::Num(
                        env.leg_values.get(&key).copied().unwrap_or(Decimal::ZERO),
                    ));
                }
            }
            match resolve_ref(path, &env.stock_set, &env.param_set) {
                Some(RefKind::Stock(p)) => {
                    check_account_open(env, &p, *span)?;
                    Ok(Value::Num(env.stocks[&p]))
                }
                Some(RefKind::Param(n)) => match env.params.get(&n) {
                    Some(ParamValue::Value(v)) => Ok(Value::Num(*v)),
                    Some(ParamValue::Error(d)) => Err(d
                        .clone()
                        .with_note(*span, format!("while evaluating param `{n}`"))),
                    Some(ParamValue::Inactive) | None => Err(Diagnostic::new(
                        *span,
                        format!(
                            "param `{n}` has no value on {}: none of its intervals cover this date",
                            env.current_date
                        ),
                    )),
                },
                None => Err(Diagnostic::new(
                    *span,
                    format!("unknown reference `{path}`"),
                )),
            }
        }
        Expr::ParamAgg(..) if locals.is_some() => Err(Diagnostic::new(
            *span,
            "aggregations are not allowed in function bodies",
        )),
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
    if env.stocks.contains_key(account) {
        return Ok(());
    }
    let open_date = env.opening_dates[account];
    let message = if open_date == env.current_date {
        format!(
            "account `{account}` is referenced on {open_date} before its opening balance is set"
        )
    } else {
        format!(
            "account `{account}` opens on {open_date}, but referenced on {}",
            env.current_date
        )
    };
    Err(Diagnostic::new(span, message))
}

/// The ledger transaction recording balances of accounts that opened on `date`.
fn opening_transaction(
    date: NaiveDate,
    opened: Vec<(Path, Decimal)>,
) -> Result<Option<Transaction>, Diagnostic> {
    let mut postings: Vec<(Path, Decimal)> =
        opened.into_iter().filter(|(_, v)| !v.is_zero()).collect();
    if postings.is_empty() {
        return Ok(None);
    }
    let equity = -checked_sum(postings.iter().map(|(_, v)| v)).ok_or_else(|| {
        Diagnostic::new(
            Span::new(0, 0),
            format!("opening balances on {date} overflow when summed"),
        )
    })?;
    postings.push((
        Path(vec!["Equity".to_string(), "OpeningBalances".to_string()]),
        equity,
    ));
    Ok(Some(Transaction {
        date,
        label: "opening-balances".to_string(),
        postings,
    }))
}

/// Rounds a value to the precision of a posted amount (cents), normalizing -0.
fn to_amount(value: Decimal) -> Decimal {
    let amount = value.round_dp(2);
    if amount.is_zero() {
        Decimal::ZERO
    } else {
        amount
    }
}

fn checked_sum<'a>(values: impl IntoIterator<Item = &'a Decimal>) -> Option<Decimal> {
    values
        .into_iter()
        .try_fold(Decimal::ZERO, |acc, v| acc.checked_add(*v))
}

fn eval_fn_body<'m>(
    fn_def: &'m FnDef,
    arg_values: &[Decimal],
    env: &Environment<'m>,
    call_span: Span,
) -> Result<Value, Diagnostic> {
    let mut scope: Locals<'m> = fn_def
        .params
        .iter()
        .zip(arg_values.iter())
        .map(|(k, &v)| (k.as_str(), v))
        .collect();

    for stmt in &fn_def.body {
        match stmt {
            Stmt::Let { name, value } => {
                let v = eval_expr(value, env, Some(&scope))?;
                match v {
                    Value::Num(n) => {
                        scope.insert(name.as_str(), n);
                    }
                    Value::Bool(_) => {
                        return Err(Diagnostic::new(
                            value.1,
                            format!("let binding `{name}` must evaluate to a number"),
                        ));
                    }
                }
            }
            Stmt::Return(expr) => {
                return eval_expr(expr, env, Some(&scope));
            }
        }
    }
    Err(Diagnostic::new(
        call_span,
        "function has no return statement",
    ))
}

fn eval_num<'m>(expr: &'m SpannedExpr, env: &Environment<'m>) -> Result<Decimal, Diagnostic> {
    match eval_expr(expr, env, None)? {
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
            let result = match op {
                BinOp::Add => x.checked_add(y),
                BinOp::Sub => x.checked_sub(y),
                BinOp::Mul => x.checked_mul(y),
                BinOp::Div => x.checked_div(y),
                _ => unreachable!(),
            };
            result.map(Value::Num).ok_or_else(|| {
                Diagnostic::new(span, format!("arithmetic overflow in `{x} {op} {y}`"))
            })
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
        BinOp::NotEq => Ok(Value::Bool(a != b)),
        BinOp::And | BinOp::Or => unreachable!("evaluated with short-circuiting in eval_expr"),
    }
}
