use crate::ast::schedule::Share;
use crate::ast::{AggKind, BinOp, Expr, ParamBody, Path, PostingAmount, Span, SpannedExpr};
use crate::compile::{Amount, Builtin, ExprKind, Function, Program, Stmt};
use crate::errors::Diagnostic;
use crate::resolver::{Model, walk_expr};
use chrono::{Datelike, Duration, NaiveDate};
use indexmap::IndexMap;
use rust_decimal::Decimal;
use std::collections::HashSet;
use std::fmt;
use std::sync::Arc;

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
    pub label: Arc<str>,
    pub postings: Vec<(Arc<Path>, Decimal)>,
}

#[derive(Debug)]
pub struct DaySnapshot {
    pub date: NaiveDate,
    /// End-of-day balance of every account, in declaration order (the order of
    /// `Output::accounts`).
    pub balances: Vec<Decimal>,
}

impl Model {
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
}

/// A param's value for the current day.
enum ParamValue {
    Value(Decimal),
    /// A time-varying param with no interval covering the current day.
    Inactive,
    /// Evaluation failed; reported only if the param is read.
    Error(Diagnostic),
}

/// Index of each period in a leg's totals.
fn period_index(kind: AggKind) -> usize {
    match kind {
        AggKind::Ytd => 0,
        AggKind::Qtd => 1,
        AggKind::Mtd => 2,
    }
}

struct State<'p> {
    program: &'p Program,
    date: NaiveDate,
    /// Balance of each account; `None` until it opens.
    balances: Vec<Option<Decimal>>,
    params: Vec<ParamValue>,
    /// Each leg's amount in the current day's firing (zero if it hasn't fired).
    legs_today: Vec<Decimal>,
    /// Each leg's year/quarter/month-to-date totals, excluding today.
    totals: Vec<[Decimal; 3]>,
    /// The entry whose postings are being evaluated.
    firing: Option<usize>,
}

impl Program {
    pub fn simulate(
        &self,
        first_day: NaiveDate,
        start: NaiveDate,
        end: NaiveDate,
    ) -> Result<SimLog, Diagnostic> {
        let mut state = State {
            program: self,
            date: first_day,
            // Accounts with no opening date are always available, starting at zero.
            balances: self
                .openings
                .iter()
                .map(|o| o.is_none().then_some(Decimal::ZERO))
                .collect(),
            params: Vec::with_capacity(self.params.len()),
            legs_today: vec![Decimal::ZERO; self.legs.len()],
            totals: vec![[Decimal::ZERO; 3]; self.legs.len()],
            firing: None,
        };
        let equity: Arc<Path> = Arc::new(Path(vec![
            "Equity".to_string(),
            "OpeningBalances".to_string(),
        ]));

        let mut log = SimLog {
            transactions: Vec::new(),
            snapshots: Vec::new(),
            opening: IndexMap::new(),
        };

        let mut t = first_day;
        while t <= end {
            state.date = t;
            state.legs_today.fill(Decimal::ZERO);
            state.reset_periods(t);
            state.evaluate_params();

            // Initialize accounts whose opening date is today. Params may read the
            // new balances, so re-evaluate them if anything opened.
            let opened = state.open_accounts()?;
            if !opened.is_empty() {
                state.evaluate_params();
            }

            // Capture opening balances at the start of the user's simulation range,
            // after any accounts that open today are initialized but before entries fire.
            if t == start {
                log.opening = self
                    .accounts
                    .iter()
                    .zip(&state.balances)
                    .map(|(p, b)| (Path::clone(p), b.unwrap_or(Decimal::ZERO)))
                    .collect();
                if checked_sum(log.opening.values()).is_none() {
                    return Err(Diagnostic::new(
                        Span::new(0, 0),
                        format!("opening balances on {t} overflow when summed"),
                    ));
                }
            }

            // Accounts opening after the start of the range get their own
            // opening transaction so ledger output agrees with the balances.
            let report = t >= start;
            if t > start {
                let opened = opened
                    .into_iter()
                    .map(|i| (self.accounts[i].clone(), state.balance(i)));
                log.transactions
                    .extend(opening_transaction(t, opened, &equity)?);
            }
            for (i, entry) in self.entries.iter().enumerate() {
                if entry.schedule.matches(t)
                    && let Some(tx) = state.fire(i)?
                    && report
                {
                    log.transactions.push(tx);
                }
            }
            state.check_assertions()?;

            if report {
                log.snapshots.push(DaySnapshot {
                    date: t,
                    balances: state
                        .balances
                        .iter()
                        .map(|b| b.unwrap_or(Decimal::ZERO))
                        .collect(),
                });
            }

            state.advance_period()?;

            t = t
                .checked_add_signed(Duration::days(1))
                .ok_or_else(|| Diagnostic::new((0..0).into(), "date overflow"))?;
        }

        Ok(log)
    }
}

impl State<'_> {
    fn balance(&self, account: usize) -> Decimal {
        self.balances[account].expect("account is open")
    }

    fn reset_periods(&mut self, t: NaiveDate) {
        if t.day() != 1 {
            return;
        }
        let new_quarter = matches!(t.month(), 1 | 4 | 7 | 10);
        let new_year = t.month() == 1;
        for totals in &mut self.totals {
            totals[period_index(AggKind::Mtd)] = Decimal::ZERO;
            if new_quarter {
                totals[period_index(AggKind::Qtd)] = Decimal::ZERO;
            }
            if new_year {
                totals[period_index(AggKind::Ytd)] = Decimal::ZERO;
            }
        }
    }

    /// Adds today's leg amounts to their period totals.
    fn advance_period(&mut self) -> Result<(), Diagnostic> {
        for (leg, (totals, &today)) in self.totals.iter_mut().zip(&self.legs_today).enumerate() {
            for total in totals.iter_mut() {
                *total = total.checked_add(today).ok_or_else(|| {
                    let leg = &self.program.legs[leg];
                    Diagnostic::new(
                        self.program.entries[leg.entry].span,
                        format!("running total of leg `{}` overflowed", leg.name),
                    )
                })?;
            }
        }
        Ok(())
    }

    /// Evaluates every param for today. Failures are recorded rather than
    /// returned, so they only abort the simulation if the param is actually read.
    fn evaluate_params(&mut self) {
        self.params.clear();
        for param in &self.program.params {
            let expr = match &param.body {
                crate::compile::ParamBody::Const(e) => Some(e),
                crate::compile::ParamBody::Intervals(intervals) => intervals
                    .iter()
                    .find(|iv| iv.contains(self.date))
                    .map(|iv| &iv.value),
            };
            let value = match expr {
                Some(e) => match self.eval_num(e, &[]) {
                    Ok(v) => ParamValue::Value(v),
                    Err(d) => ParamValue::Error(d),
                },
                None => ParamValue::Inactive,
            };
            self.params.push(value);
        }
    }

    /// Initializes accounts whose opening date is today, in declaration order,
    /// and returns their indices.
    fn open_accounts(&mut self) -> Result<Vec<usize>, Diagnostic> {
        let mut opened = Vec::new();
        for (account, opening) in self.program.openings.iter().enumerate() {
            if let Some((expr, date)) = opening
                && *date == self.date
            {
                self.balances[account] = Some(to_amount(self.eval_num(expr, &[])?));
                opened.push(account);
            }
        }
        Ok(opened)
    }

    /// Fires entry `index`, returning its transaction unless it moved no money.
    fn fire(&mut self, index: usize) -> Result<Option<Transaction>, Diagnostic> {
        let program = self.program;
        let entry = &program.entries[index];
        self.firing = Some(index);
        let mut postings: Vec<(Arc<Path>, Decimal)> = Vec::with_capacity(entry.postings.len());
        let mut amounts: Vec<(usize, Decimal)> = Vec::with_capacity(entry.postings.len());
        let mut auto = None;

        // Every posting sees the balances from before the entry fired.
        for posting in &entry.postings {
            self.check_open(posting.account, posting.account_span)?;
            let amount =
                match &posting.amount {
                    Amount::Expr(e) => to_amount(self.eval_num(e, &[]).map_err(|d| {
                        d.with_note(entry.span, format!("in entry `{}`", entry.label))
                    })?),
                    Amount::All => to_amount(-self.balance(posting.account)),
                    Amount::Auto => {
                        auto = Some(posting);
                        continue;
                    }
                };
            if let Some(leg) = posting.leg {
                self.legs_today[leg] = amount;
            }
            amounts.push((posting.account, amount));
        }

        let sum = checked_sum(amounts.iter().map(|(_, a)| a)).ok_or_else(|| {
            Diagnostic::new(
                entry.span,
                format!("postings of entry `{}` overflow when summed", entry.label),
            )
        })?;
        match auto {
            Some(posting) => {
                // Exact negation guarantees the transaction sums to zero.
                let amount = to_amount(-sum);
                if let Some(leg) = posting.leg {
                    self.legs_today[leg] = amount;
                }
                amounts.push((posting.account, amount));
            }
            None if !sum.is_zero() => {
                return Err(Diagnostic::new(
                    entry.span,
                    format!(
                        "entry `{}` does not balance on {}: postings sum to {sum}",
                        entry.label, self.date
                    ),
                ));
            }
            None => {}
        }

        for &(account, amount) in &amounts {
            let balance = self.balance(account);
            self.balances[account] = Some(balance.checked_add(amount).ok_or_else(|| {
                Diagnostic::new(
                    entry.span,
                    format!("balance of `{}` overflowed", program.accounts[account]),
                )
            })?);
            postings.push((program.accounts[account].clone(), amount));
        }

        // An entry that moves no money (e.g. `= all` on an empty account) is
        // not worth a ledger transaction.
        if amounts.iter().all(|(_, amount)| amount.is_zero()) {
            return Ok(None);
        }
        Ok(Some(Transaction {
            date: self.date,
            label: entry.label.clone(),
            postings,
        }))
    }

    fn check_assertions(&self) -> Result<(), Diagnostic> {
        for (schedule, expr) in &self.program.asserts {
            if !schedule.matches(self.date) {
                continue;
            }
            match self.eval(expr, &[])? {
                Value::Bool(true) => {}
                Value::Bool(false) => {
                    let t = self.date;
                    let msg = match &expr.kind {
                        ExprKind::Bin(lhs, op, rhs) => {
                            match (self.eval(lhs, &[]), self.eval(rhs, &[])) {
                                (Ok(lv), Ok(rv)) => {
                                    format!("assertion failed on {t}: {lv} {op} {rv}")
                                }
                                _ => format!("assertion failed on {t}"),
                            }
                        }
                        _ => format!("assertion failed on {t}"),
                    };
                    return Err(Diagnostic::new(expr.span, msg));
                }
                Value::Num(_) => {
                    return Err(Diagnostic::new(
                        expr.span,
                        "assertion expression must evaluate to a bool",
                    ));
                }
            }
        }
        Ok(())
    }

    fn check_open(&self, account: usize, span: Span) -> Result<(), Diagnostic> {
        if self.balances[account].is_some() {
            return Ok(());
        }
        let name = &self.program.accounts[account];
        let (_, open_date) = self.program.openings[account]
            .as_ref()
            .expect("accounts without an opening are always open");
        let message = if *open_date == self.date {
            format!(
                "account `{name}` is referenced on {open_date} before its opening balance is set"
            )
        } else {
            format!(
                "account `{name}` opens on {open_date}, but referenced on {}",
                self.date
            )
        };
        Err(Diagnostic::new(span, message))
    }

    fn eval_num(
        &self,
        expr: &crate::compile::Expr,
        locals: &[Decimal],
    ) -> Result<Decimal, Diagnostic> {
        match self.eval(expr, locals)? {
            Value::Num(n) => Ok(n),
            Value::Bool(_) => Err(Diagnostic::new(
                expr.span,
                "expected a numeric value, got bool",
            )),
        }
    }

    /// Evaluates `expr`; `locals` holds the current function's parameters and
    /// `let` bindings (empty outside functions).
    fn eval(&self, expr: &crate::compile::Expr, locals: &[Decimal]) -> Result<Value, Diagnostic> {
        let span = expr.span;
        match &expr.kind {
            ExprKind::Num(n) => Ok(Value::Num(*n)),
            ExprKind::Bool(b) => Ok(Value::Bool(*b)),
            ExprKind::Account(account) => {
                self.check_open(*account, span)?;
                Ok(Value::Num(self.balance(*account)))
            }
            ExprKind::Param(param) => match &self.params[*param] {
                ParamValue::Value(v) => Ok(Value::Num(*v)),
                ParamValue::Error(d) => Err(d.clone().with_note(
                    span,
                    format!(
                        "while evaluating param `{}`",
                        self.program.params[*param].name
                    ),
                )),
                ParamValue::Inactive => Err(Diagnostic::new(
                    span,
                    format!(
                        "param `{}` has no value on {}: none of its intervals cover this date",
                        self.program.params[*param].name, self.date
                    ),
                )),
            },
            ExprKind::Leg(leg) => Ok(Value::Num(self.legs_today[*leg])),
            ExprKind::Local(slot) => Ok(Value::Num(locals[*slot])),
            ExprKind::Total(leg, kind) => Ok(Value::Num(self.totals[*leg][period_index(*kind)])),
            ExprKind::Neg(x) => match self.eval(x, locals)? {
                Value::Num(n) => Ok(Value::Num(-n)),
                _ => Err(Diagnostic::new(
                    span,
                    "unary minus requires a numeric operand",
                )),
            },
            ExprKind::Not(x) => match self.eval(x, locals)? {
                Value::Bool(b) => Ok(Value::Bool(!b)),
                _ => Err(Diagnostic::new(span, "`not` requires a bool operand")),
            },
            ExprKind::Bin(a, op @ (BinOp::And | BinOp::Or), b) => {
                let as_bool = |operand: &crate::compile::Expr, v| match v {
                    Value::Bool(b) => Ok(b),
                    Value::Num(_) => Err(Diagnostic::new(
                        operand.span,
                        format!("operands of `{op}` must be bools"),
                    )),
                };
                let x = as_bool(a, self.eval(a, locals)?)?;
                // Short-circuit: the right side is only evaluated when needed.
                if (*op == BinOp::And && !x) || (*op == BinOp::Or && x) {
                    return Ok(Value::Bool(x));
                }
                Ok(Value::Bool(as_bool(b, self.eval(b, locals)?)?))
            }
            ExprKind::Bin(a, op, b) => {
                let x = self.eval(a, locals)?;
                let y = self.eval(b, locals)?;
                apply_binop(*op, x, y, span)
            }
            ExprKind::If(cond, then, else_) => match self.eval(cond, locals)? {
                Value::Bool(true) => self.eval(then, locals),
                Value::Bool(false) => self.eval(else_, locals),
                _ => Err(Diagnostic::new(
                    span,
                    "condition in `if` expression must be a bool",
                )),
            },
            ExprKind::Builtin(builtin, args) => {
                let arg = |i: usize| self.numeric_arg(&args[i], builtin.name(), locals);
                let n = match builtin {
                    Builtin::Min => arg(0)?.min(arg(1)?),
                    Builtin::Max => arg(0)?.max(arg(1)?),
                    Builtin::Abs => arg(0)?.abs(),
                    Builtin::Floor => arg(0)?.floor(),
                    Builtin::Ceil => arg(0)?.ceil(),
                    Builtin::Round => arg(0)?.round(),
                };
                Ok(Value::Num(n))
            }
            ExprKind::Call(index, args) => {
                let function = &self.program.fns[*index];
                let mut frame = vec![Decimal::ZERO; function.slots];
                for (slot, arg) in args.iter().enumerate() {
                    frame[slot] = self.numeric_arg(arg, &function.name, locals)?;
                }
                self.call(function, frame, span)
            }
            ExprKind::Per(e, _) => self.eval(e, locals),
            ExprKind::Spread(e, unit) => {
                let amount = self.eval_num(e, locals)?;
                let entry = self.firing.expect("only postings spread amounts");
                let share = self.program.entries[entry].schedule.share(*unit, self.date);
                spread(amount, share).map(Value::Num).ok_or_else(|| {
                    Diagnostic::new(
                        span,
                        format!("arithmetic overflow spreading {amount} per {unit}"),
                    )
                })
            }
        }
    }

    fn numeric_arg(
        &self,
        arg: &crate::compile::Expr,
        callee: &str,
        locals: &[Decimal],
    ) -> Result<Decimal, Diagnostic> {
        match self.eval(arg, locals)? {
            Value::Num(n) => Ok(n),
            _ => Err(Diagnostic::new(
                arg.span,
                format!("argument to `{callee}` must be numeric"),
            )),
        }
    }

    /// Runs `function` with its arguments already in the first slots of `frame`.
    fn call(
        &self,
        function: &Function,
        mut frame: Vec<Decimal>,
        call_span: Span,
    ) -> Result<Value, Diagnostic> {
        debug_assert!(function.arity <= frame.len());
        for stmt in &function.body {
            match stmt {
                Stmt::Let { slot, name, value } => match self.eval(value, &frame)? {
                    Value::Num(n) => frame[*slot] = n,
                    Value::Bool(_) => {
                        return Err(Diagnostic::new(
                            value.span,
                            format!("let binding `{name}` must evaluate to a number"),
                        ));
                    }
                },
                Stmt::Return(e) => return self.eval(e, &frame),
            }
        }
        Err(Diagnostic::new(
            call_span,
            "function has no return statement",
        ))
    }
}

/// The ledger transaction recording balances of accounts that opened on `date`.
fn opening_transaction(
    date: NaiveDate,
    opened: impl Iterator<Item = (Arc<Path>, Decimal)>,
    equity: &Arc<Path>,
) -> Result<Option<Transaction>, Diagnostic> {
    let mut postings: Vec<(Arc<Path>, Decimal)> = opened.filter(|(_, v)| !v.is_zero()).collect();
    if postings.is_empty() {
        return Ok(None);
    }
    let total = -checked_sum(postings.iter().map(|(_, v)| v)).ok_or_else(|| {
        Diagnostic::new(
            Span::new(0, 0),
            format!("opening balances on {date} overflow when summed"),
        )
    })?;
    postings.push((equity.clone(), total));
    Ok(Some(Transaction {
        date,
        label: "opening-balances".into(),
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

/// A firing's part of `amount` per period, in cents. Each firing in a period
/// gets the difference between two rounded running totals, so a period's
/// firings add up to exactly its amount.
fn spread(amount: Decimal, share: Share) -> Option<Decimal> {
    debug_assert!(share.index >= 1 && share.index <= share.count);
    let running = |firings: u32| {
        let total = amount
            .checked_mul(firings.into())?
            .checked_div(share.count.into())?;
        Some(total.round_dp(2))
    };
    let carried = amount.checked_mul(share.carried.into())?.round_dp(2);
    carried
        .checked_add(running(share.index)?)?
        .checked_sub(running(share.index - 1)?)
}

fn checked_sum<'a>(values: impl IntoIterator<Item = &'a Decimal>) -> Option<Decimal> {
    values
        .into_iter()
        .try_fold(Decimal::ZERO, |acc, v| acc.checked_add(*v))
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
