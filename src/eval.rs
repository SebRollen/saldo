use crate::ast::schedule::Share;
use crate::ast::{
    AggKind, BinOp, Dist, EntryStmt, Expr, GrowthKind, Path, Span, SpannedExpr, TimeUnit,
};
use crate::compile::{Builtin, ExprKind, Function, Growth, Program, Stmt};
use crate::errors::Diagnostic;
use crate::resolver::{Counted, Model, ParamBody, walk_expr};
use chrono::{Datelike, Duration, NaiveDate};
use indexmap::IndexMap;
use rust_decimal::Decimal;
use rust_decimal::prelude::{FromPrimitive, ToPrimitive};
use std::collections::{HashMap, HashSet};
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
    /// The checks that failed on a reported day, in the order they're
    /// written.
    pub failed_checks: Vec<CheckFailure>,
}

/// What a simulation keeps, besides each reported day's balances.
#[derive(Clone, Copy)]
pub struct Record<'a> {
    /// Snapshots are of the last day of each of these in the report, and of
    /// its last day.
    pub every: TimeUnit,
    /// The values snapshots record, by index.
    pub values: &'a [usize],
    /// Whether to keep transactions, which the ledger lists.
    pub transactions: bool,
}

/// A check that failed on some of the days reported.
#[derive(Debug, Clone)]
pub struct CheckFailure {
    /// Where the check is written.
    pub span: Span,
    /// The first day it failed.
    pub first: NaiveDate,
    /// What it compared that day, like `-1371.42 >= 0`, if it's a comparison.
    pub found: Option<String>,
    /// How many days it failed on.
    pub days: usize,
    /// The last day it failed.
    pub last: NaiveDate,
}

impl CheckFailure {
    /// Says when the check failed, pointing at it.
    pub fn diagnostic(&self) -> Diagnostic {
        self.diagnostic_in(None)
    }

    /// Says when the check failed, in `scenario` if it's one of several,
    /// pointing at it.
    pub fn diagnostic_in(&self, scenario: Option<&str>) -> Diagnostic {
        let mut message = match scenario {
            Some(scenario) => format!("check failed in {scenario} on {}", self.first),
            None => format!("check failed on {}", self.first),
        };
        if let Some(found) = &self.found {
            message = format!("{message}: {found}");
        }
        if self.days > 1 {
            let more = self.days - 1;
            let days = if more == 1 { "day" } else { "days" };
            message = format!(
                "{message}, and on {more} more {days}, last on {}",
                self.last
            );
        }
        Diagnostic::new(self.span, message)
    }
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
    /// End-of-day ledger balance of every account, in the order of
    /// `Output::accounts`.
    pub balances: Vec<Decimal>,
    /// The values asked for, in the order asked, or `None` for one that has
    /// no value that day.
    pub values: Vec<Option<Decimal>>,
}

impl Model {
    /// The first simulated day when reporting from `start`: the earliest
    /// opening date if that's before `start`, so the warm-up period accumulates
    /// the right balances before reporting starts.
    pub fn first_day(&self, start: NaiveDate) -> NaiveDate {
        self.accounts
            .values()
            .filter_map(|a| a.opening.as_ref().map(|(_, d)| *d))
            .min()
            .map_or(start, |earliest| earliest.min(start))
    }

    /// Warns about totals that are missing amounts because the simulation
    /// starts partway through their period, after an entry they count would
    /// already have fired.
    pub fn missed_aggregate_warnings(&self, first_day: NaiveDate) -> Vec<Diagnostic> {
        struct Aggregate {
            /// What it counts: a tag's entry, or an account or group.
            counts: Counted,
            kind: AggKind,
            /// How it's written, e.g. `paycheck.k401[year]`.
            display: String,
            span: Span,
        }
        let mut aggregates: Vec<Aggregate> = Vec::new();
        let mut collect = |e: &SpannedExpr| {
            walk_expr(e, &mut |sub| match sub.0.as_ref() {
                Expr::TagTotal { entry, tag, agg } => aggregates.push(Aggregate {
                    counts: Counted::Tag(entry.clone(), tag.clone()),
                    kind: *agg,
                    display: format!("{entry}.{tag}{agg}"),
                    span: sub.1,
                }),
                Expr::AccountTotal(path, agg) => aggregates.push(Aggregate {
                    counts: Counted::Account(path.clone()),
                    kind: *agg,
                    display: format!("{path}{agg}"),
                    span: sub.1,
                }),
                _ => {}
            });
        };
        for body in self.params.values() {
            match body {
                ParamBody::Const(e) => collect(e),
                ParamBody::Arms(arms) => {
                    for arm in arms {
                        collect(&arm.value);
                    }
                }
            }
        }
        for entry in &self.entries {
            EntryStmt::visit(&entry.body, &mut |stmt| match stmt {
                EntryStmt::Let { value, .. } => collect(value),
                EntryStmt::Move(m) => collect(&m.amount),
                EntryStmt::If { branches, .. } => {
                    for (cond, _) in branches {
                        collect(cond);
                    }
                }
                EntryStmt::For { .. } => unreachable!("the resolver expands loops"),
            });
        }
        for (_, e) in &self.checks {
            collect(e);
        }
        aggregates.sort_by_key(|a| a.span.start);

        let mut warned = HashSet::new();
        let mut warnings = Vec::new();
        for agg in aggregates {
            if !warned.insert((agg.counts.clone(), agg.kind)) {
                continue;
            }
            // A `balances` block may say what it was then.
            let period = agg.kind.period_start(first_day);
            if self.totals_so_far.iter().any(|s| {
                s.counted == agg.counts
                    && s.agg == agg.kind
                    && agg.kind.period_start(s.date) == period
            }) {
                continue;
            }
            let counted = self.entries.iter().filter(|e| match &agg.counts {
                Counted::Tag(entry, _) => e.name == *entry,
                Counted::Account(path) => {
                    let mut moves = false;
                    EntryStmt::visit(&e.body, &mut |stmt| {
                        moves |= matches!(stmt, EntryStmt::Move(m)
                            if m.from.0.starts_with(path) || m.to.0.starts_with(path));
                    });
                    moves
                }
            });
            let period_start = agg.kind.period_start(first_day);
            let missed = counted
                .filter_map(|entry| {
                    let day = period_start
                        .iter_days()
                        .take_while(|d| *d < first_day)
                        .find(|d| entry.schedule.matches(*d))?;
                    Some((day, entry))
                })
                .min_by_key(|(day, _)| *day);
            if let Some((missed, entry)) = missed {
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
                            "`{}` would have fired on {missed}; simulate from {period_start}, \
                             or say what was moved by then with a total in a `balances` block",
                            entry.label
                        ),
                    ),
                );
            }
        }
        warnings
    }
}

impl Program {
    /// Warns about limits and `fill`s that can't know what their move moved
    /// earlier in a period, because the simulation starts partway through it.
    pub fn missed_fill_warnings(&self, first_day: NaiveDate) -> Vec<Diagnostic> {
        self.fills
            .iter()
            .filter_map(|fill| {
                let entry = &self.entries[fill.entry];
                let period_start = fill.unit.period_start(first_day);
                // A `balances` block may say what its tag had moved then.
                let set_for_fill = |so_far: &crate::compile::TotalSoFar| {
                    matches!(so_far.counted, crate::compile::Counted::Tag(tag) if Some(tag) == fill.tag)
                        && so_far.agg.unit() == fill.unit
                        && fill.unit.period_start(so_far.date) == period_start
                };
                if self.totals_so_far.iter().any(set_for_fill) {
                    return None;
                }
                let missed = period_start
                    .iter_days()
                    .take_while(|d| *d < first_day)
                    .find(|d| entry.schedule.matches(*d))?;
                Some(
                    Diagnostic::new(
                        fill.span,
                        format!("what's left here doesn't count what was moved before {first_day}"),
                    )
                    .with_note(
                        fill.span,
                        format!(
                            "`{}` would have fired on {missed}; simulate from {period_start}, \
                             or say what was moved by then with a total in a `balances` block",
                            entry.label
                        ),
                    ),
                )
            })
            .collect()
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

/// Index of each period in a tag's or account's totals.
fn period_index(kind: AggKind) -> usize {
    match kind {
        AggKind::Ytd => 0,
        AggKind::Qtd => 1,
        AggKind::Mtd => 2,
    }
}

struct State<'p> {
    program: &'p Program,
    /// What every random draw depends on, with where it's written and when.
    seed: u64,
    date: NaiveDate,
    /// Ledger balance of each account; `None` until it opens.
    balances: Vec<Option<Decimal>>,
    params: Vec<ParamValue>,
    /// What each tag's moves moved today (zero if its entry hasn't fired).
    tags_today: Vec<Decimal>,
    /// Each tag's year/quarter/month-to-date totals, excluding today; see
    /// `total` for what an expression reads.
    totals: Vec<[Decimal; 3]>,
    /// What moved into each account today, as a ledger amount.
    accounts_today: Vec<Decimal>,
    /// What moved into each account so far this year, quarter and month,
    /// excluding today.
    account_totals: Vec<[Decimal; 3]>,
    /// The entry whose moves are being evaluated.
    firing: Option<usize>,
    /// For each `fill`, the start of the period it last moved in and what its
    /// move has moved since.
    fills: Vec<(NaiveDate, Decimal)>,
    /// For each growing arm, by param and arm, how many steps it has taken
    /// and what they come to.
    grown: HashMap<(usize, usize), (u32, Decimal)>,
    /// For each check, when it has failed on reported days.
    failed_checks: Vec<Option<CheckFailure>>,
}

impl Program {
    /// Simulates from `first_day` and reports from `start` to `end`, drawing
    /// with `seed`, keeping what `record` says.
    pub fn simulate(
        &self,
        first_day: NaiveDate,
        (start, end): (NaiveDate, NaiveDate),
        seed: u64,
        record: &Record,
    ) -> Result<SimLog, Diagnostic> {
        let Record {
            every,
            values,
            transactions,
        } = *record;
        let mut state = State {
            program: self,
            seed,
            date: first_day,
            // Accounts with no opening date are always available, starting at zero.
            balances: self
                .openings
                .iter()
                .map(|o| o.is_none().then_some(Decimal::ZERO))
                .collect(),
            params: Vec::with_capacity(self.params.len()),
            tags_today: vec![Decimal::ZERO; self.tags.len()],
            totals: vec![[Decimal::ZERO; 3]; self.tags.len()],
            accounts_today: vec![Decimal::ZERO; self.accounts.len()],
            account_totals: vec![[Decimal::ZERO; 3]; self.accounts.len()],
            firing: None,
            fills: vec![(NaiveDate::MIN, Decimal::ZERO); self.fills.len()],
            grown: HashMap::new(),
            failed_checks: vec![None; self.checks.len()],
        };
        let equity: Arc<Path> = Arc::new(Path(vec![
            "equity".to_string(),
            "opening_balances".to_string(),
        ]));

        let mut log = SimLog {
            transactions: Vec::new(),
            snapshots: Vec::new(),
            opening: IndexMap::new(),
            failed_checks: Vec::new(),
        };

        let mut t = first_day;
        while t <= end {
            state.date = t;
            state.tags_today.fill(Decimal::ZERO);
            state.accounts_today.fill(Decimal::ZERO);
            state.reset_periods(t);
            state.evaluate_params();

            // Initialize accounts whose opening date is today, and the
            // totals that start then.
            let opened = state.open_accounts()?;
            state.set_totals()?;

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
            if t > start && transactions {
                let opened = opened
                    .into_iter()
                    .map(|i| (self.accounts[i].clone(), state.balance(i)));
                log.transactions
                    .extend(opening_transaction(t, opened, &equity)?);
            }
            for (i, entry) in self.entries.iter().enumerate() {
                if !entry.schedule.matches(t) {
                    continue;
                }
                // Later entries and today's checks see what this one moved.
                let tx = state.fire(i)?;
                if let Some(tx) = tx
                    && report
                    && transactions
                {
                    log.transactions.push(tx);
                }
            }
            state.run_checks(report)?;

            let last_of_period = t
                .succ_opt()
                .is_none_or(|next| every.period_start(next) != every.period_start(t));
            if report && (last_of_period || t == end) {
                log.snapshots.push(DaySnapshot {
                    date: t,
                    balances: state
                        .balances
                        .iter()
                        .map(|b| b.unwrap_or(Decimal::ZERO))
                        .collect(),
                    values: values
                        .iter()
                        .map(|&param| match &state.params[param] {
                            ParamValue::Value(v) => Ok(Some(*v)),
                            ParamValue::Inactive => Ok(None),
                            ParamValue::Error(diag) => Err(diag.clone()),
                        })
                        .collect::<Result<_, _>>()?,
                });
            }

            state.advance_period()?;

            t = t
                .checked_add_signed(Duration::days(1))
                .ok_or_else(|| Diagnostic::new((0..0).into(), "date overflow"))?;
        }

        log.failed_checks = state.failed_checks.into_iter().flatten().collect();
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
        for totals in self.totals.iter_mut().chain(&mut self.account_totals) {
            totals[period_index(AggKind::Mtd)] = Decimal::ZERO;
            if new_quarter {
                totals[period_index(AggKind::Qtd)] = Decimal::ZERO;
            }
            if new_year {
                totals[period_index(AggKind::Ytd)] = Decimal::ZERO;
            }
        }
    }

    /// Adds today's tag and account amounts to their period totals.
    fn advance_period(&mut self) -> Result<(), Diagnostic> {
        for (tag, (totals, &today)) in self.totals.iter_mut().zip(&self.tags_today).enumerate() {
            for total in totals.iter_mut() {
                *total = total.checked_add(today).ok_or_else(|| {
                    let tag = &self.program.tags[tag];
                    Diagnostic::new(
                        self.program.entries[tag.entry].span,
                        format!("running total of tag `{}` overflowed", tag.name),
                    )
                })?;
            }
        }
        let accounts = self.account_totals.iter_mut().zip(&self.accounts_today);
        for (account, (totals, &today)) in accounts.enumerate() {
            for total in totals.iter_mut() {
                *total = total.checked_add(today).ok_or_else(|| {
                    Diagnostic::new(
                        Span::new(0, 0),
                        format!(
                            "running total of `{}` overflowed",
                            self.program.accounts[account]
                        ),
                    )
                })?;
            }
        }
        Ok(())
    }

    /// Evaluates every value for today. Values depend only on the date, so
    /// this happens once a day. Failures are recorded rather than returned,
    /// so they only abort the simulation if the value is actually read.
    fn evaluate_params(&mut self) {
        self.params.clear();
        let program = self.program;
        for (index, param) in program.params.iter().enumerate() {
            let value = match &param.body {
                crate::compile::ParamBody::Const(e) => Some(self.eval_num(e, &[])),
                crate::compile::ParamBody::Arms(arms) => arms
                    .iter()
                    .position(|arm| arm.when.contains(self.date))
                    .map(|i| {
                        let base = self.eval_num(&arms[i].value, &[])?;
                        match &arms[i].growth {
                            Some(growth) => self.grow((index, i), growth, base),
                            None => Ok(base),
                        }
                    }),
            };
            self.params.push(match value {
                Some(Ok(v)) => ParamValue::Value(v),
                Some(Err(d)) => ParamValue::Error(d),
                None => ParamValue::Inactive,
            });
        }
    }

    /// `base` grown by the steps `growth` has taken by today. Days only move
    /// forward, so each arm's steps so far are kept and only new ones taken.
    fn grow(
        &mut self,
        arm: (usize, usize),
        growth: &Growth,
        base: Decimal,
    ) -> Result<Decimal, Diagnostic> {
        let steps = growth.every.steps(growth.anchor, self.date);
        let (taken, total) = match self.grown.get(&arm) {
            Some(&(taken, total)) if taken <= steps => (taken, total),
            _ => (0, no_growth(growth)),
        };
        let total = self.take_steps(growth, taken, steps, total)?;
        self.grown.insert(arm, (steps, total));
        apply_growth(growth, base, total)
    }

    /// `total` after `growth`'s steps after the `taken`th up to the `steps`th,
    /// each at the rate on the day it's taken.
    fn take_steps(
        &self,
        growth: &Growth,
        taken: u32,
        steps: u32,
        mut total: Decimal,
    ) -> Result<Decimal, Diagnostic> {
        for n in taken + 1..=steps {
            let on = growth
                .every
                .after(growth.anchor, n)
                .expect("a step that's been taken has a date");
            let by = self.eval_num_at(&growth.by, &[], on)?;
            total = match growth.kind {
                GrowthKind::Compounding => Decimal::ONE
                    .checked_add(by)
                    .and_then(|f| f.checked_mul(total)),
                GrowthKind::Adding => total.checked_add(by),
            }
            .ok_or_else(|| {
                Diagnostic::new(growth.by.span, "arithmetic overflow growing a value")
            })?;
        }
        Ok(total)
    }

    /// The value of `param` on `date`, which values can be evaluated on
    /// because they depend on nothing but the date.
    fn param_on(&self, param: usize, date: NaiveDate, span: Span) -> Result<Decimal, Diagnostic> {
        let name = &self.program.params[param].name;
        match &self.program.params[param].body {
            crate::compile::ParamBody::Const(e) => self.eval_num_at(e, &[], date),
            crate::compile::ParamBody::Arms(arms) => {
                let Some(arm) = arms.iter().find(|arm| arm.when.contains(date)) else {
                    return Err(Diagnostic::new(
                        span,
                        format!(
                            "`{name}` has no value on {date}: none of its arms cover this date"
                        ),
                    ));
                };
                let base = self.eval_num_at(&arm.value, &[], date)?;
                let Some(growth) = &arm.growth else {
                    return Ok(base);
                };
                let steps = growth.every.steps(growth.anchor, date);
                let total = self.take_steps(growth, 0, steps, no_growth(growth))?;
                apply_growth(growth, base, total)
            }
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
                let balance = to_amount(self.eval_num(expr, &[])?) * self.program.signs[account];
                self.balances[account] = Some(balance);
                opened.push(account);
            }
        }
        Ok(opened)
    }

    /// Sets the totals that `balances` blocks give for today.
    fn set_totals(&mut self) -> Result<(), Diagnostic> {
        for so_far in &self.program.totals_so_far {
            if so_far.date != self.date {
                continue;
            }
            let value = to_amount(self.eval_num(&so_far.value, &[])?);
            let period = period_index(so_far.agg);
            match so_far.counted {
                crate::compile::Counted::Tag(tag) => self.totals[tag][period] = value,
                crate::compile::Counted::Account(account, sign) => {
                    self.account_totals[account][period] = value * sign;
                }
            }
        }
        Ok(())
    }

    /// Fires entry `index`, returning its transaction unless it moved no money.
    fn fire(&mut self, index: usize) -> Result<Option<Transaction>, Diagnostic> {
        let program = self.program;
        let entry = &program.entries[index];
        self.firing = Some(index);
        let mut frame = vec![Decimal::ZERO; entry.slots];
        // What each account receives, in the order they're first named.
        let mut net: IndexMap<usize, Decimal> = IndexMap::new();
        // Every move sees the balances from before the entry fired.
        self.run(&entry.body, &mut frame, &mut net)
            .map_err(|d| d.with_note(entry.span, format!("in entry `{}`", entry.label)))?;
        self.firing = None;

        let mut postings: Vec<(Arc<Path>, Decimal)> = Vec::with_capacity(net.len());
        for (&account, &amount) in &net {
            let overflow = || {
                Diagnostic::new(
                    entry.span,
                    format!("balance of `{}` overflowed", program.accounts[account]),
                )
            };
            let balance = self.balance(account);
            self.balances[account] = Some(balance.checked_add(amount).ok_or_else(overflow)?);
            self.accounts_today[account] = self.accounts_today[account]
                .checked_add(amount)
                .ok_or_else(overflow)?;
            if !amount.is_zero() {
                postings.push((program.accounts[account].clone(), amount));
            }
        }

        // An entry that moves no money (e.g. clearing an empty account) is
        // not worth a ledger transaction.
        if postings.is_empty() {
            return Ok(None);
        }
        Ok(Some(Transaction {
            date: self.date,
            label: entry.label.clone(),
            postings,
        }))
    }

    /// Runs `stmts`, statements of the firing entry: works out its `let`s
    /// into `frame`, and adds what its moves move to `net`.
    fn run(
        &mut self,
        stmts: &[crate::compile::EntryStmt],
        frame: &mut Vec<Decimal>,
        net: &mut IndexMap<usize, Decimal>,
    ) -> Result<(), Diagnostic> {
        let program = self.program;
        for stmt in stmts {
            let m = match stmt {
                crate::compile::EntryStmt::Let { slot, value } => {
                    frame[*slot] = self.eval_num(value, frame)?;
                    continue;
                }
                crate::compile::EntryStmt::If {
                    branches,
                    otherwise,
                } => {
                    let mut taken = otherwise;
                    for (cond, body) in branches {
                        match self.eval(cond, frame)? {
                            Value::Bool(true) => {
                                taken = body;
                                break;
                            }
                            Value::Bool(false) => {}
                            Value::Num(_) => {
                                return Err(Diagnostic::new(
                                    cond.span,
                                    "a condition must be a bool",
                                ));
                            }
                        }
                    }
                    self.run(taken, frame, net)?;
                    continue;
                }
                crate::compile::EntryStmt::Move(m) => m,
            };
            self.check_open(m.from, m.from_span)?;
            self.check_open(m.to, m.to_span)?;
            let amount = to_amount(self.eval_num(&m.amount, frame)?);
            let overflow = || Diagnostic::new(m.span, "this move's amounts overflowed");
            if let Some(tag) = m.tag {
                self.tags_today[tag] = self.tags_today[tag]
                    .checked_add(amount)
                    .ok_or_else(overflow)?;
            }
            for &fill in &m.fills {
                let start = program.fills[fill].unit.period_start(self.date);
                let (period, moved) = &mut self.fills[fill];
                if *period != start {
                    (*period, *moved) = (start, Decimal::ZERO);
                }
                *moved = moved.checked_add(amount).ok_or_else(overflow)?;
            }
            let to = net.entry(m.to).or_default();
            *to = to.checked_add(amount).ok_or_else(overflow)?;
            let from = net.entry(m.from).or_default();
            *from = from.checked_sub(amount).ok_or_else(overflow)?;
        }
        Ok(())
    }

    /// Runs today's checks, recording those that fail if today is reported.
    /// A failing check doesn't stop the simulation; an error in one does.
    fn run_checks(&mut self, report: bool) -> Result<(), Diagnostic> {
        let program = self.program;
        for (i, (schedule, expr)) in program.checks.iter().enumerate() {
            if !schedule.matches(self.date) {
                continue;
            }
            match self.eval(expr, &[])? {
                Value::Bool(true) => {}
                Value::Bool(false) if report => {
                    let t = self.date;
                    if let Some(failure) = &mut self.failed_checks[i] {
                        failure.days += 1;
                        failure.last = t;
                        continue;
                    }
                    // What it compared, the first day it failed.
                    let found = match &expr.kind {
                        ExprKind::Bin(lhs, op, rhs) => {
                            match (self.eval(lhs, &[]), self.eval(rhs, &[])) {
                                (Ok(lv), Ok(rv)) => Some(format!("{lv} {op} {rv}")),
                                _ => None,
                            }
                        }
                        _ => None,
                    };
                    self.failed_checks[i] = Some(CheckFailure {
                        span: expr.span,
                        first: t,
                        found,
                        days: 1,
                        last: t,
                    });
                }
                Value::Bool(false) => {}
                Value::Num(_) => {
                    return Err(Diagnostic::new(expr.span, "a check must be a bool"));
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
        self.eval_num_at(expr, locals, self.date)
    }

    fn eval_num_at(
        &self,
        expr: &crate::compile::Expr,
        locals: &[Decimal],
        date: NaiveDate,
    ) -> Result<Decimal, Diagnostic> {
        match self.eval_at(expr, locals, date)? {
            Value::Num(n) => Ok(n),
            Value::Bool(_) => Err(Diagnostic::new(
                expr.span,
                "expected a numeric value, got bool",
            )),
        }
    }

    /// Evaluates `expr` today; `locals` holds the current function's or
    /// entry's parameters and `let`s.
    fn eval(&self, expr: &crate::compile::Expr, locals: &[Decimal]) -> Result<Value, Diagnostic> {
        self.eval_at(expr, locals, self.date)
    }

    /// Evaluates `expr` on `date`. Only what depends on nothing but the date,
    /// like values, can be evaluated on a day other than today.
    fn eval_at(
        &self,
        expr: &crate::compile::Expr,
        locals: &[Decimal],
        date: NaiveDate,
    ) -> Result<Value, Diagnostic> {
        let span = expr.span;
        let eval = |e| self.eval_at(e, locals, date);
        match &expr.kind {
            ExprKind::Num(n) => Ok(Value::Num(*n)),
            ExprKind::Bool(b) => Ok(Value::Bool(*b)),
            ExprKind::Balance {
                accounts,
                sign,
                group,
            } => self
                .balance_of(accounts, *sign, *group, span)
                .map(Value::Num),
            ExprKind::Param(param) if date == self.date => self.param(*param, span),
            ExprKind::Param(param) => self.param_on(*param, date, span).map(Value::Num),
            ExprKind::Local(slot) => Ok(Value::Num(locals[*slot])),
            ExprKind::Total(tag, kind) => self.total(*tag, *kind, span).map(Value::Num),
            ExprKind::AccountTotal {
                accounts,
                sign,
                agg,
            } => self
                .account_total(accounts, *sign, *agg, span)
                .map(Value::Num),
            ExprKind::Neg(x) => match eval(x)? {
                Value::Num(n) => Ok(Value::Num(-n)),
                _ => Err(Diagnostic::new(
                    span,
                    "unary minus requires a numeric operand",
                )),
            },
            ExprKind::Not(x) => match eval(x)? {
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
                let x = as_bool(a, eval(a)?)?;
                // Short-circuit: the right side is only evaluated when needed.
                if (*op == BinOp::And && !x) || (*op == BinOp::Or && x) {
                    return Ok(Value::Bool(x));
                }
                Ok(Value::Bool(as_bool(b, eval(b)?)?))
            }
            ExprKind::Bin(a, op, b) => {
                let x = eval(a)?;
                let y = eval(b)?;
                apply_binop(*op, x, y, span)
            }
            ExprKind::If(cond, then, else_) => match eval(cond)? {
                Value::Bool(true) => eval(then),
                Value::Bool(false) => eval(else_),
                _ => Err(Diagnostic::new(
                    span,
                    "condition in `if` expression must be a bool",
                )),
            },
            ExprKind::Builtin(builtin, args) => self.builtin(*builtin, args, locals, span, date),
            ExprKind::Call(index, args) => {
                let function = &self.program.fns[*index];
                let mut frame = vec![Decimal::ZERO; function.slots];
                for (slot, arg) in args.iter().enumerate() {
                    frame[slot] = self.numeric_arg(arg, &function.name, locals, date)?;
                }
                self.call(function, frame, span, date)
            }
            ExprKind::Per(e, _) | ExprKind::Limit(e, _) => eval(e),
            ExprKind::Fill(e, unit, fill) => {
                let target = self.eval_num(e, locals)?;
                self.fill(target, *unit, *fill, span).map(Value::Num)
            }
            ExprKind::Spread(e, unit) => self.spread(e, *unit, locals, span).map(Value::Num),
            ExprKind::Draw(dist, args, slot) => self.draw(*dist, args, *slot, locals, span, date),
        }
    }

    /// A draw from `dist` for `date`, the same for every day of the period
    /// it's drawn for.
    fn draw(
        &self,
        dist: Dist,
        args: &[crate::compile::Expr],
        slot: usize,
        locals: &[Decimal],
        span: Span,
        date: NaiveDate,
    ) -> Result<Value, Diagnostic> {
        let draw = &self.program.draws[slot];
        let period = draw.period.period_start(date).num_days_from_ce();
        let bits = crate::random::mix(
            crate::random::mix(self.seed, draw.site),
            u64::from(period.unsigned_abs()),
        );
        let u = crate::random::unit_interval(bits);
        let decimal = |x: f64| Decimal::from_f64(x).expect("a draw is a finite number");
        let arg = |i: usize| self.eval_num_at(&args[i], locals, date);
        let overflow =
            || Diagnostic::new(span, format!("arithmetic overflow in `{}`", dist.name()));
        match dist {
            Dist::Normal => {
                let (mean, sd) = (arg(0)?, arg(1)?);
                if sd < Decimal::ZERO {
                    return Err(Diagnostic::new(
                        args[1].span,
                        format!("a standard deviation can't be negative, but this is {sd}"),
                    ));
                }
                let z = decimal(crate::random::normal_quantile(u));
                sd.checked_mul(z)
                    .and_then(|x| mean.checked_add(x))
                    .map(Value::Num)
                    .ok_or_else(overflow)
            }
            Dist::Lognormal => {
                let (mean, sd) = (arg(0)?, arg(1)?);
                if sd < Decimal::ZERO {
                    return Err(Diagnostic::new(
                        args[1].span,
                        format!("a standard deviation can't be negative, but this is {sd}"),
                    ));
                }
                if mean <= Decimal::ZERO {
                    return Err(Diagnostic::new(
                        args[0].span,
                        format!("a lognormal's mean is more than 0, but this is {mean}"),
                    ));
                }
                if sd.is_zero() {
                    return Ok(Value::Num(mean));
                }
                let (Some(m), Some(s)) = (mean.to_f64(), sd.to_f64()) else {
                    return Err(overflow());
                };
                let x = crate::random::lognormal(m, s, crate::random::normal_quantile(u));
                Decimal::from_f64(x).map(Value::Num).ok_or_else(overflow)
            }
            Dist::Uniform => {
                let (lo, hi) = (arg(0)?, arg(1)?);
                if lo > hi {
                    return Err(Diagnostic::new(
                        span,
                        format!("`uniform` draws from {lo} up to {hi}, but {lo} is more than {hi}"),
                    ));
                }
                hi.checked_sub(lo)
                    .and_then(|width| width.checked_mul(decimal(u)))
                    .and_then(|x| lo.checked_add(x))
                    .map(Value::Num)
                    .ok_or_else(overflow)
            }
            Dist::Chance => {
                let p = arg(0)?;
                if p < Decimal::ZERO || p > Decimal::ONE {
                    return Err(Diagnostic::new(
                        args[0].span,
                        format!("a chance is between 0% and 100%, but this is {p}"),
                    ));
                }
                Ok(Value::Bool(decimal(u) < p))
            }
        }
    }

    // `eval` recurses once per level of nesting, so its arms with much to do
    // call these to keep its stack frame small.

    /// The sum of `accounts`' balances, read as their kind reads by
    /// multiplying by `sign`. A group counts accounts that haven't opened as
    /// zero.
    fn balance_of(
        &self,
        accounts: &[usize],
        sign: Decimal,
        group: bool,
        span: Span,
    ) -> Result<Decimal, Diagnostic> {
        let mut sum = Decimal::ZERO;
        for &account in accounts {
            let balance = match self.balances[account] {
                Some(balance) => balance,
                None if group => continue,
                None => return Err(self.check_open(account, span).unwrap_err()),
            };
            sum = sum
                .checked_add(balance)
                .ok_or_else(|| Diagnostic::new(span, "this sum of balances overflowed"))?;
        }
        Ok(sum * sign)
    }

    /// What moved into `accounts` so far this `kind` of period, including
    /// earlier entries today, read as their kind reads.
    fn account_total(
        &self,
        accounts: &[usize],
        sign: Decimal,
        kind: AggKind,
        span: Span,
    ) -> Result<Decimal, Diagnostic> {
        let overflow = || Diagnostic::new(span, "this running total overflowed");
        let mut sum = Decimal::ZERO;
        for &account in accounts {
            let total = self.account_totals[account][period_index(kind)]
                .checked_add(self.accounts_today[account])
                .ok_or_else(overflow)?;
            sum = sum.checked_add(total).ok_or_else(overflow)?;
        }
        Ok(sum * sign)
    }

    /// What `tag`'s moves have moved so far this `kind` of period: earlier
    /// days, plus today unless its entry is the one firing.
    fn total(&self, tag: usize, kind: AggKind, span: Span) -> Result<Decimal, Diagnostic> {
        let earlier = self.totals[tag][period_index(kind)];
        if self.firing == Some(self.program.tags[tag].entry) {
            return Ok(earlier);
        }
        earlier.checked_add(self.tags_today[tag]).ok_or_else(|| {
            Diagnostic::new(
                span,
                format!(
                    "running total of tag `{}` overflowed",
                    self.program.tags[tag].name
                ),
            )
        })
    }

    fn param(&self, param: usize, span: Span) -> Result<Value, Diagnostic> {
        match &self.params[param] {
            ParamValue::Value(v) => Ok(Value::Num(*v)),
            ParamValue::Error(d) => Err(d.clone().with_note(
                span,
                format!("while evaluating `{}`", self.program.params[param].name),
            )),
            ParamValue::Inactive => Err(Diagnostic::new(
                span,
                format!(
                    "`{}` has no value on {}: none of its arms cover this date",
                    self.program.params[param].name, self.date
                ),
            )),
        }
    }

    fn builtin(
        &self,
        builtin: Builtin,
        args: &[crate::compile::Expr],
        locals: &[Decimal],
        span: Span,
        date: NaiveDate,
    ) -> Result<Value, Diagnostic> {
        let arg = |i: usize| self.numeric_arg(&args[i], builtin.name(), locals, date);
        let n = match builtin {
            Builtin::Min => arg(0)?.min(arg(1)?),
            Builtin::Max => arg(0)?.max(arg(1)?),
            Builtin::Abs => arg(0)?.abs(),
            Builtin::Floor => arg(0)?.floor(),
            Builtin::Ceil => arg(0)?.ceil(),
            Builtin::Round => arg(0)?.round(),
            Builtin::Fill => {
                return Err(Diagnostic::new(
                    span,
                    "`fill` can only be the amount a move moves",
                ));
            }
        };
        Ok(Value::Num(n))
    }

    /// The firing entry's share of `expr`, an amount per `unit`. `locals`
    /// are the entry's `let`s.
    ///
    /// The periods that roll into the firing each add their own amount, the
    /// value of `expr` on the period's last day, if it depends on nothing but
    /// the date. Otherwise they get today's, which is an error if a value it
    /// reads changes during them.
    fn spread(
        &self,
        expr: &crate::compile::Expr,
        unit: TimeUnit,
        locals: &[Decimal],
        span: Span,
    ) -> Result<Decimal, Diagnostic> {
        let amount = self.eval_num(expr, locals)?;
        let entry = &self.program.entries[self.firing.expect("only entries spread amounts")];
        let share = entry.schedule.share(unit, self.date);
        let overflow = || {
            Diagnostic::new(
                span,
                format!("arithmetic overflow spreading {amount} per {unit}"),
            )
        };
        let mut carried = Decimal::ZERO;
        if share.carried > 0 && dated(expr, entry) {
            let mut slots = Vec::new();
            locals_in(expr, entry, &mut slots);
            for start in entry.schedule.carried_periods(unit, self.date) {
                let last = unit
                    .next_period_start(start)
                    .and_then(|next| next.pred_opt())
                    .expect("a period before today ends");
                let frame = self.frame_on(entry, locals, &slots, last)?;
                let own = self.eval_num_at(expr, &frame, last)?;
                carried = carried.checked_add(own).ok_or_else(overflow)?;
            }
        } else if share.carried > 0 {
            self.check_carried(expr, entry, unit, span)?;
            carried = amount
                .checked_mul(share.carried.into())
                .ok_or_else(overflow)?;
        }
        spread(amount, carried, share).ok_or_else(overflow)
    }

    /// `entry`'s `let`s on `date`: `locals`, with those in `slots`, which
    /// depend on nothing but the date, worked out again for `date`.
    fn frame_on(
        &self,
        entry: &crate::compile::Entry,
        locals: &[Decimal],
        slots: &[usize],
        date: NaiveDate,
    ) -> Result<Vec<Decimal>, Diagnostic> {
        let mut frame = locals.to_vec();
        let mut lets = Vec::new();
        crate::compile::EntryStmt::visit(&entry.body, &mut |stmt| {
            if let crate::compile::EntryStmt::Let { slot, value } = stmt
                && slots.contains(slot)
            {
                lets.push((*slot, value));
            }
        });
        for (slot, value) in lets {
            frame[slot] = self.eval_num_at(value, &frame, date)?;
        }
        Ok(frame)
    }

    /// Errors if a value `expr` reads takes a different arm on a day of the
    /// periods that roll into today's firing, which get its value today.
    fn check_carried(
        &self,
        expr: &crate::compile::Expr,
        entry: &crate::compile::Entry,
        unit: TimeUnit,
        span: Span,
    ) -> Result<(), Diagnostic> {
        let carried = entry.schedule.carried_periods(unit, self.date);
        let own = unit.period_start(self.date);
        let carried_days = || carried.iter().flat_map(|p| unit.days(*p));
        let fire_more_often = |diag: Diagnostic| {
            let unit = unit.name();
            Err(diag.with_note(
                entry.span,
                format!("this fires less often than once a {unit}; fire it at least once a {unit}"),
            ))
        };
        for param in self.params_read(expr, entry) {
            let param = &self.program.params[param];
            // Which arm applies, how many steps it has grown, and which
            // period a new draw in it is for.
            let state = |t: NaiveDate| {
                let arm = match &param.body {
                    crate::compile::ParamBody::Const(_) => Some((0, 0)),
                    crate::compile::ParamBody::Arms(arms) => {
                        let i = arms.iter().position(|arm| arm.when.contains(t))?;
                        let steps = arms[i]
                            .growth
                            .as_ref()
                            .map_or(0, |g| g.every.steps(g.anchor, t));
                        Some((i, steps))
                    }
                };
                Some((arm, param.draws_every.map(|every| every.period_start(t))))
            };
            let today = state(self.date);
            if carried_days().any(|d| state(d) != today) {
                return fire_more_often(Diagnostic::new(
                    span,
                    format!(
                        "`{}` changes during the {}s before {own} that `{}` covers, \
                         but this reads balances or totals too, so they'd all get its value on {}",
                        param.name,
                        unit.name(),
                        entry.label,
                        self.date
                    ),
                ));
            }
        }
        let mut draws = Vec::new();
        draws_in(expr, entry, &mut draws);
        for draw in draws {
            let every = self.program.draws[draw].period;
            let today = every.period_start(self.date);
            if carried_days().any(|d| every.period_start(d) != today) {
                return fire_more_often(
                    Diagnostic::new(
                        span,
                        format!(
                            "this draws anew during the {}s before {own} that `{}` covers, \
                             but reads balances or totals too, so they'd all get its draw for {}",
                            unit.name(),
                            entry.label,
                            self.date
                        ),
                    )
                    .with_note(
                        self.program.draws[draw].span,
                        format!("drawn every {every}"),
                    ),
                );
            }
        }
        Ok(())
    }

    /// The values `expr` reads, directly, through other values, or through
    /// `entry`'s `let`s.
    fn params_read(
        &self,
        expr: &crate::compile::Expr,
        entry: &crate::compile::Entry,
    ) -> Vec<usize> {
        let mut found = Vec::new();
        params_in(expr, entry, &mut found);
        let mut i = 0;
        while let Some(&param) = found.get(i) {
            let mut more = Vec::new();
            match &self.program.params[param].body {
                crate::compile::ParamBody::Const(e) => params_in(e, entry, &mut more),
                crate::compile::ParamBody::Arms(arms) => {
                    for arm in arms {
                        params_in(&arm.value, entry, &mut more);
                    }
                }
            }
            for p in more {
                if !found.contains(&p) {
                    found.push(p);
                }
            }
            i += 1;
        }
        found
    }

    /// What's left of `target` this `unit`, split over the firings left in it.
    fn fill(
        &self,
        target: Decimal,
        unit: TimeUnit,
        fill: usize,
        span: Span,
    ) -> Result<Decimal, Diagnostic> {
        let entry = self.firing.expect("only entries fill");
        let start = unit.period_start(self.date);
        // What's been moved this period: the total of the move's tag, which a
        // `balances` block can set, or else what the move has moved.
        let posted = match (self.program.fills[fill].tag, AggKind::from_unit(unit)) {
            (Some(tag), Some(agg)) => self.total(tag, agg, span)?,
            _ => match self.fills[fill] {
                (period, posted) if period == start => posted,
                _ => Decimal::ZERO,
            },
        };
        // The schedule fires today, so this is at least 1.
        let left = self.program.entries[entry]
            .schedule
            .firings_left(unit, self.date);
        target
            .checked_sub(posted)
            .and_then(|rest| rest.checked_div(left.into()))
            .map(|amount| amount.round_dp(2))
            .ok_or_else(|| Diagnostic::new(span, format!("arithmetic overflow filling {target}")))
    }

    fn numeric_arg(
        &self,
        arg: &crate::compile::Expr,
        callee: &str,
        locals: &[Decimal],
        date: NaiveDate,
    ) -> Result<Decimal, Diagnostic> {
        match self.eval_at(arg, locals, date)? {
            Value::Num(n) => Ok(n),
            _ => Err(Diagnostic::new(
                arg.span,
                format!("argument to `{callee}` must be numeric"),
            )),
        }
    }

    /// Runs `function` on `date`, with its arguments already in the first
    /// slots of `frame`.
    fn call(
        &self,
        function: &Function,
        mut frame: Vec<Decimal>,
        call_span: Span,
        date: NaiveDate,
    ) -> Result<Value, Diagnostic> {
        debug_assert!(function.arity <= frame.len());
        for stmt in &function.body {
            match stmt {
                Stmt::Let { slot, name, value } => match self.eval_at(value, &frame, date)? {
                    Value::Num(n) => frame[*slot] = n,
                    Value::Bool(_) => {
                        return Err(Diagnostic::new(
                            value.span,
                            format!("let binding `{name}` must evaluate to a number"),
                        ));
                    }
                },
                Stmt::Return(e) => return self.eval_at(e, &frame, date),
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

/// What no steps of `growth` come to: nothing added, or a factor of one.
fn no_growth(growth: &Growth) -> Decimal {
    match growth.kind {
        GrowthKind::Compounding => Decimal::ONE,
        GrowthKind::Adding => Decimal::ZERO,
    }
}

/// `base` grown by `total`, what `growth`'s steps come to.
fn apply_growth(growth: &Growth, base: Decimal, total: Decimal) -> Result<Decimal, Diagnostic> {
    match growth.kind {
        GrowthKind::Compounding => base.checked_mul(total),
        GrowthKind::Adding => base.checked_add(total),
    }
    .ok_or_else(|| Diagnostic::new(growth.by.span, "arithmetic overflow growing a value"))
}

/// The `let` of `entry` that fills `slot`.
fn let_value(entry: &crate::compile::Entry, slot: usize) -> &crate::compile::Expr {
    let mut found = None;
    crate::compile::EntryStmt::visit(&entry.body, &mut |stmt| {
        if let crate::compile::EntryStmt::Let { slot: s, value } = stmt
            && *s == slot
        {
            found = Some(value);
        }
    });
    found.expect("every slot has a `let`")
}

/// Adds the values `e` reads directly, or through `entry`'s `let`s, to `out`.
fn params_in(e: &crate::compile::Expr, entry: &crate::compile::Entry, out: &mut Vec<usize>) {
    let mut visit = |e: &crate::compile::Expr| params_in(e, entry, out);
    match &e.kind {
        ExprKind::Param(param) => out.push(*param),
        ExprKind::Local(slot) => visit(let_value(entry, *slot)),
        ExprKind::Num(_)
        | ExprKind::Bool(_)
        | ExprKind::Balance { .. }
        | ExprKind::AccountTotal { .. }
        | ExprKind::Total(..) => {}
        ExprKind::Neg(x)
        | ExprKind::Not(x)
        | ExprKind::Per(x, _)
        | ExprKind::Limit(x, _)
        | ExprKind::Spread(x, _)
        | ExprKind::Fill(x, ..) => visit(x),
        ExprKind::Bin(a, _, b) => {
            visit(a);
            visit(b);
        }
        ExprKind::If(c, t, f) => {
            visit(c);
            visit(t);
            visit(f);
        }
        ExprKind::Builtin(_, args) | ExprKind::Call(_, args) | ExprKind::Draw(_, args, _) => {
            args.iter().for_each(visit)
        }
    }
}

/// Adds the slots of the `let`s of `entry` that `e` reads, directly or
/// through other `let`s, to `out`.
fn locals_in(e: &crate::compile::Expr, entry: &crate::compile::Entry, out: &mut Vec<usize>) {
    if let ExprKind::Local(slot) = e.kind {
        if !out.contains(&slot) {
            out.push(slot);
            locals_in(let_value(entry, slot), entry, out);
        }
        return;
    }
    let mut visit = |e: &crate::compile::Expr| locals_in(e, entry, out);
    match &e.kind {
        ExprKind::Local(_) => unreachable!("handled above"),
        ExprKind::Num(_)
        | ExprKind::Bool(_)
        | ExprKind::Param(_)
        | ExprKind::Balance { .. }
        | ExprKind::AccountTotal { .. }
        | ExprKind::Total(..) => {}
        ExprKind::Neg(x)
        | ExprKind::Not(x)
        | ExprKind::Per(x, _)
        | ExprKind::Limit(x, _)
        | ExprKind::Spread(x, _)
        | ExprKind::Fill(x, ..) => visit(x),
        ExprKind::Bin(a, _, b) => {
            visit(a);
            visit(b);
        }
        ExprKind::If(c, t, f) => {
            visit(c);
            visit(t);
            visit(f);
        }
        ExprKind::Builtin(_, args) | ExprKind::Call(_, args) | ExprKind::Draw(_, args, _) => {
            args.iter().for_each(visit)
        }
    }
}

/// Adds the draws in `e`, or in the `let`s of `entry` it reads, to `out`.
fn draws_in(e: &crate::compile::Expr, entry: &crate::compile::Entry, out: &mut Vec<usize>) {
    if let ExprKind::Draw(_, _, slot) = e.kind {
        out.push(slot);
    }
    let mut visit = |e: &crate::compile::Expr| draws_in(e, entry, out);
    match &e.kind {
        ExprKind::Local(slot) => visit(let_value(entry, *slot)),
        ExprKind::Num(_)
        | ExprKind::Bool(_)
        | ExprKind::Param(_)
        | ExprKind::Balance { .. }
        | ExprKind::AccountTotal { .. }
        | ExprKind::Total(..) => {}
        ExprKind::Neg(x)
        | ExprKind::Not(x)
        | ExprKind::Per(x, _)
        | ExprKind::Limit(x, _)
        | ExprKind::Spread(x, _)
        | ExprKind::Fill(x, ..) => visit(x),
        ExprKind::Bin(a, _, b) => {
            visit(a);
            visit(b);
        }
        ExprKind::If(c, t, f) => {
            visit(c);
            visit(t);
            visit(f);
        }
        ExprKind::Builtin(_, args) | ExprKind::Call(_, args) | ExprKind::Draw(_, args, _) => {
            args.iter().for_each(visit)
        }
    }
}

/// Whether `e` depends on nothing but the date, through `entry`'s `let`s too,
/// so it can be evaluated on another day.
fn dated(e: &crate::compile::Expr, entry: &crate::compile::Entry) -> bool {
    let dated = |e: &crate::compile::Expr| dated(e, entry);
    match &e.kind {
        ExprKind::Num(_) | ExprKind::Bool(_) | ExprKind::Param(_) => true,
        ExprKind::Local(slot) => dated(let_value(entry, *slot)),
        ExprKind::Balance { .. }
        | ExprKind::AccountTotal { .. }
        | ExprKind::Total(..)
        | ExprKind::Spread(..)
        | ExprKind::Fill(..)
        | ExprKind::Builtin(Builtin::Fill, _) => false,
        ExprKind::Neg(x) | ExprKind::Not(x) | ExprKind::Per(x, _) | ExprKind::Limit(x, _) => {
            dated(x)
        }
        ExprKind::Bin(a, _, b) => dated(a) && dated(b),
        ExprKind::If(c, t, f) => dated(c) && dated(t) && dated(f),
        // Functions only read their arguments and values, and a draw depends
        // only on the date, given the seed.
        ExprKind::Builtin(_, args) | ExprKind::Call(_, args) | ExprKind::Draw(_, args, _) => {
            args.iter().all(dated)
        }
    }
}

/// A firing's part of `amount` per period, in cents: the difference between
/// two running totals rounded to cents, counted from the start of the periods
/// that roll into its own, which come to `carried`. Each firing is rounded
/// once, and a period's firings add up to exactly its amount.
fn spread(amount: Decimal, carried: Decimal, share: Share) -> Option<Decimal> {
    debug_assert!(share.index >= 1 && share.index <= share.count);
    // The running total through the `parts`th of `count` parts of the
    // firing's own period. The first firing starts from nothing; the others
    // start where the previous one ended.
    let through = |parts: u32| {
        let own = amount
            .checked_mul(parts.into())?
            .checked_div(share.count.into())?;
        Some(carried.checked_add(own)?.round_dp(2))
    };
    let start = match share.index {
        1 => Decimal::ZERO,
        index => through(index - 1)?,
    };
    through(share.index)?.checked_sub(start)
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
    ("fill", 1),
    ("normal", 2),
    ("lognormal", 2),
    ("uniform", 2),
    ("chance", 1),
];

/// `x` to the power `y`, or `None` if it's too large. A whole power multiplies
/// exactly; any other goes through `f64`, which is as close as an `f64` gets.
fn power(x: Decimal, y: Decimal) -> Option<Decimal> {
    if !y.fract().is_zero() {
        let result = x.to_f64()?.powf(y.to_f64()?);
        return if result.is_finite() {
            Decimal::from_f64(result)
        } else {
            None
        };
    }
    // By squaring: `x` to each power of two, times those that make up `y`.
    let (mut result, mut square, mut n) = (Decimal::ONE, x, y.abs().to_u64()?);
    while n > 0 {
        if n & 1 == 1 {
            result = result.checked_mul(square)?;
        }
        n >>= 1;
        if n > 0 {
            square = square.checked_mul(square)?;
        }
    }
    if y.is_sign_negative() {
        Decimal::ONE.checked_div(result)
    } else {
        Some(result)
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
        BinOp::Pow => {
            let (Value::Num(x), Value::Num(y)) = (a, b) else {
                return Err(Diagnostic::new(span, "`^` requires numeric operands"));
            };
            if x.is_zero() && y.is_sign_negative() {
                return Err(Diagnostic::new(span, "division by zero"));
            }
            if x.is_sign_negative() && !y.fract().is_zero() {
                return Err(Diagnostic::new(
                    span,
                    format!("`{x} ^ {y}` has no value: a negative number has no fractional power"),
                ));
            }
            power(x, y)
                .map(Value::Num)
                .ok_or_else(|| Diagnostic::new(span, format!("arithmetic overflow in `{x} ^ {y}`")))
        }
        BinOp::Eq => Ok(Value::Bool(a == b)),
        BinOp::NotEq => Ok(Value::Bool(a != b)),
        BinOp::And | BinOp::Or => unreachable!("evaluated with short-circuiting in eval_expr"),
        BinOp::Union | BinOp::Inter => unreachable!("the resolver rejects sets of days"),
    }
}
