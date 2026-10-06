mod ast;
mod compile;
mod errors;
mod eval;
mod expand;
mod lexer;
mod parser;
mod random;
mod resolver;
mod scenario;
mod sources;
mod typecheck;
mod units;
mod util;

use chrono::NaiveDate;
use rust_decimal::Decimal;
use std::io;
use std::sync::Arc;

pub use ast::{Path, Span, TimeUnit};
pub use errors::{Diagnostic, Error};
pub use eval::{CheckFailure, DaySnapshot, SimLog, Transaction, Value};
pub use sources::Sources;

#[derive(Clone)]
pub struct RunOpts {
    pub from: NaiveDate,
    pub to: NaiveDate,
    /// What random draws depend on: the same seed draws the same numbers.
    pub seed: u64,
    /// CSV reports the last day of each of these, from `from` to `to`.
    pub every: TimeUnit,
    /// What CSV reports, by name: accounts, groups of them or values. Empty
    /// for every account.
    pub series: Vec<String>,
}

#[derive(Debug)]
pub struct Output {
    /// Every account, ordered by path.
    pub accounts: Vec<Path>,
    pub log: SimLog,
    /// Problems that don't stop the simulation but may make results wrong.
    pub warnings: Vec<Diagnostic>,
    /// Whether the model draws at random, so that what it does depends on
    /// the seed.
    pub draws: bool,
    /// What CSV reports.
    columns: Vec<Column>,
}

/// A column of CSV output.
#[derive(Debug, Clone)]
struct Column {
    name: String,
    of: Measure,
}

#[derive(Debug, Clone)]
enum Measure {
    /// The sum of these accounts' ledger balances, times the sign that reads
    /// their kind positive.
    Accounts(Vec<usize>, Decimal),
    /// The value a snapshot records at this index.
    Value(usize),
    /// A value the model doesn't have, which is always empty.
    Missing,
}

impl Output {
    pub fn to_ledger(&self) -> String {
        let mut out = Vec::new();
        self.write_ledger(&mut out)
            .expect("writing to a Vec can't fail");
        String::from_utf8(out).expect("ledger output is UTF-8")
    }

    pub fn to_csv(&self) -> String {
        let mut out = Vec::new();
        self.write_csv(&mut out)
            .expect("writing to a Vec can't fail");
        String::from_utf8(out).expect("CSV output is UTF-8")
    }

    /// Writes double-entry transactions in ledger format.
    pub fn write_ledger(&self, out: &mut impl io::Write) -> io::Result<()> {
        emit_ledger(out, &self.accounts, &self.log)
    }

    /// Writes the series asked for as CSV, one row per day reported, with
    /// accounts read as each kind reads: liabilities and income positive.
    pub fn write_csv(&self, out: &mut impl io::Write) -> io::Result<()> {
        emit_csv(out, &self.columns, &self.log, None, None, true)
    }

    /// Writes CSV as [`Output::write_csv`] does, for run `number` of many,
    /// which drew with `seed`: each row starts with them, and the header, if
    /// `header`, names them `run` and `seed`.
    pub fn write_run_csv(
        &self,
        out: &mut impl io::Write,
        number: u32,
        seed: u64,
        header: bool,
    ) -> io::Result<()> {
        emit_csv(
            out,
            &self.columns,
            &self.log,
            Some((number, seed)),
            None,
            header,
        )
    }

    /// Writes CSV as [`Output::write_csv`] does, for scenario `scenario` of
    /// several, and for `run`'s number and seed if it's one of many runs:
    /// each row starts with them, and the header, if `header`, names them
    /// `run`, `seed` and `scenario`.
    pub fn write_scenario_csv(
        &self,
        out: &mut impl io::Write,
        scenario: &str,
        run: Option<(u32, u64)>,
        header: bool,
    ) -> io::Result<()> {
        emit_csv(out, &self.columns, &self.log, run, Some(scenario), header)
    }
}

/// One of many runs: its number, from 1, the seed it drew with, and what it
/// did.
pub struct Run {
    pub number: u32,
    pub seed: u64,
    /// Which of the models side by side it's a run of, from 0.
    pub scenario: usize,
    pub result: Result<Output, Vec<Error>>,
}

/// A model that's been read and checked, ready to simulate as often as wanted.
#[derive(Clone)]
pub struct Model {
    model: resolver::Model,
    program: compile::Program,
    /// The model as written, and its scenarios, to apply them to.
    source: Arc<ast::Program>,
    /// The scenarios applied to it, by their places in `source`.
    applied: Vec<usize>,
}

/// Reads and checks a model given as text. It can't have modules in other
/// files; use [`load_file`] for that.
pub fn load(src: &str) -> Result<Model, Vec<Error>> {
    let program = sources::load_text(&mut Sources::default(), src)?;
    Model::new(program)
}

/// Reads and checks the model in the file at `path`, reading the files of its
/// modules. `sources` collects the text of every file read, for
/// [`Sources::format_errors`] and [`Sources::format_warnings`].
pub fn load_file(
    path: impl AsRef<std::path::Path>,
    sources: &mut Sources,
) -> Result<Model, Vec<Error>> {
    let program = sources::load_file(sources, path.as_ref())?;
    Model::new(program)
}

/// Simulates a model given as text. It can't have modules in other files; use
/// [`run_file`] for that.
pub fn run(src: &str, opts: &RunOpts) -> Result<Output, Vec<Error>> {
    check_range(opts)?;
    load(src)?.simulate(opts)
}

/// Simulates the model in the file at `path`, reading the files of its
/// modules. `sources` collects the text of every file read, for
/// [`Sources::format_errors`] and [`Sources::format_warnings`].
pub fn run_file(
    path: impl AsRef<std::path::Path>,
    opts: &RunOpts,
    sources: &mut Sources,
) -> Result<Output, Vec<Error>> {
    check_range(opts)?;
    load_file(path, sources)?.simulate(opts)
}

fn check_range(opts: &RunOpts) -> Result<(), Vec<Error>> {
    if opts.from > opts.to {
        return Err(vec![Error::InvalidDateRange {
            from: opts.from,
            to: opts.to,
        }]);
    }
    Ok(())
}

impl Model {
    /// The model `source` describes, without its scenarios.
    fn new(source: ast::Program) -> Result<Self, Vec<Error>> {
        let diags = scenario::check_names(&source);
        if !diags.is_empty() {
            return Err(diags.into_iter().map(Error::Diagnostic).collect());
        }
        let source = Arc::new(source);
        Self::build(&source, &[])
            .map_err(|diags| diags.into_iter().map(Error::Diagnostic).collect())
    }

    /// The model `source` describes, with its scenarios at `applied`, one
    /// after another.
    fn build(source: &Arc<ast::Program>, applied: &[usize]) -> Result<Self, Vec<Diagnostic>> {
        let scenarios: Vec<&ast::Scenario> =
            applied.iter().map(|&i| &source.scenarios[i]).collect();
        let (program, changes) = scenario::apply(source, &scenarios)?;
        let model = resolver::resolve(&program, &changes)?;
        let mut program = compile::compile(&model);
        let diags = units::check(&mut program);
        if !diags.is_empty() {
            return Err(diags);
        }
        Ok(Model {
            model,
            program,
            source: Arc::clone(source),
            applied: applied.to_vec(),
        })
    }

    /// The names of the model's scenarios, in the order they're declared.
    pub fn scenarios(&self) -> impl Iterator<Item = &str> {
        self.source.scenarios.iter().map(|s| s.name.0.as_str())
    }

    /// The model as it's written with scenario `name`, or with the scenarios
    /// `name` joins with `+`, one after another. `base` is the model as it's
    /// written.
    pub fn scenario(&self, name: &str) -> Result<Model, Vec<Error>> {
        let mut applied = Vec::new();
        let mut errors = Vec::new();
        for part in name.split('+').map(str::trim) {
            if part == scenario::BASE {
                continue;
            }
            match self.scenarios().position(|s| s == part) {
                Some(i) => applied.push(i),
                None => errors.push(Error::UnknownScenario {
                    name: part.to_string(),
                    close: util::closest(part, self.scenarios().chain([scenario::BASE]))
                        .map(str::to_string),
                }),
            }
        }
        if !errors.is_empty() {
            return Err(errors);
        }
        if applied == self.applied {
            return Ok(self.clone());
        }
        Self::build(&self.source, &applied)
            .map_err(|diags| named_errors(&self.source, &applied, diags))
    }

    /// The scenarios `names`, each as [`Model::scenario`] takes it, to
    /// simulate side by side, reporting `series` as CSV.
    pub fn side_by_side(
        &self,
        names: &[String],
        series: &[String],
    ) -> Result<SideBySide, Vec<Error>> {
        let mut models = Vec::new();
        let mut errors = Vec::new();
        for name in names {
            match self.scenario(name) {
                Ok(model) => models.push((name.clone(), model)),
                Err(e) => errors.extend(e),
            }
        }
        if !errors.is_empty() {
            return Err(errors);
        }
        SideBySide::new(models, series)
    }

    /// Whether the model draws at random, so that what it does depends on
    /// the seed.
    pub fn draws(&self) -> bool {
        !self.program.draws.is_empty()
    }

    /// Simulates the model as `opts` says.
    pub fn simulate(&self, opts: &RunOpts) -> Result<Output, Vec<Error>> {
        let columns = self.columns(&opts.series)?;
        self.simulate_keeping(opts, true, &columns)
    }

    /// The seed that run `number` of many draws with, when `seed` is the
    /// seed of them all.
    pub fn run_seed(seed: u64, number: u32) -> u64 {
        random::mix(seed, number.into())
    }

    /// Simulates runs 1 to `runs`, each drawing with its own seed (see
    /// [`Model::run_seed`]), on every core, and hands each to `each` in order
    /// of number. Stops when `each` returns `false`. Runs don't keep their
    /// transactions; simulate a run's seed for its ledger.
    pub fn simulate_runs(&self, opts: &RunOpts, runs: u32, mut each: impl FnMut(Run) -> bool) {
        match SideBySide::new(vec![(String::new(), self.clone())], &opts.series) {
            Ok(models) => models.simulate_runs(opts, runs, each),
            Err(errors) => {
                each(Run {
                    number: 1,
                    seed: Self::run_seed(opts.seed, 1),
                    scenario: 0,
                    result: Err(errors),
                });
            }
        }
    }

    fn simulate_keeping(
        &self,
        opts: &RunOpts,
        transactions: bool,
        (columns, values): &Columns,
    ) -> Result<Output, Vec<Error>> {
        check_range(opts)?;
        let first_day = self.model.first_day(opts.from);
        let record = eval::Record {
            every: opts.every,
            values,
            transactions,
        };
        let log = self
            .program
            .simulate(first_day, (opts.from, opts.to), opts.seed, &record)
            .map_err(|mut d| {
                // What went wrong may depend on what was drawn.
                if self.draws() {
                    d.message = format!("{} (drawing with seed {})", d.message, opts.seed);
                }
                named_errors(&self.source, &self.applied, vec![d])
            })?;

        let accounts = self.model.accounts.keys().cloned().collect();
        let mut warnings = self.model.warnings.clone();
        warnings.extend(self.model.missed_aggregate_warnings(first_day));
        warnings.extend(self.program.missed_fill_warnings(first_day));
        warnings.sort_by_key(|d| d.span.start);

        Ok(Output {
            accounts,
            log,
            warnings,
            draws: self.draws(),
            columns: columns.clone(),
        })
    }

    /// The columns for `series`, or for every account if it's empty, and the
    /// values snapshots have to record for them.
    fn columns(&self, series: &[String]) -> Result<Columns, Vec<Error>> {
        let mut columns = shared_columns(&[self], series)?;
        Ok(columns.pop().expect("a model has columns"))
    }

    /// What column `name` of CSV reports, if it names something in the model:
    /// an account, the sum of a group of accounts, or a value, which is added
    /// to the `values` snapshots record.
    fn measure(&self, name: &str, values: &mut Vec<usize>) -> Option<Measure> {
        let accounts = &self.program.accounts;
        let path = Path(name.split('.').map(str::to_string).collect());
        let under: Vec<usize> = (0..accounts.len())
            .filter(|&i| accounts[i].starts_with(&path))
            .collect();
        if path.account_kind().is_some() && !under.is_empty() {
            return Some(Measure::Accounts(under, sign(&path)));
        }
        let param = self.program.params.iter().position(|p| p.name == *name)?;
        values.push(param);
        Some(Measure::Value(values.len() - 1))
    }

    /// The name closest to `name` that a column of CSV can report.
    fn closest_series(&self, name: &str) -> Option<String> {
        let names: Vec<String> = self
            .program
            .accounts
            .iter()
            .map(|a| a.to_string())
            .chain(self.program.params.iter().map(|p| p.name.clone()))
            .collect();
        util::closest(name, names.iter().map(String::as_str)).map(str::to_string)
    }
}

/// The columns of CSV for a model, and the values its snapshots record for
/// them.
type Columns = (Vec<Column>, Vec<usize>);

/// The sign that reads an account of `path`'s kind positive.
fn sign(path: &Path) -> Decimal {
    path.account_kind().expect("accounts have kinds").sign()
}

/// What CSV reports for each of `models`: the same columns, for `series`, or
/// for every account in any of them if it's empty, each read from the model's
/// own accounts and values. An account a model doesn't have reads 0, and a
/// value it doesn't have is empty.
fn shared_columns(models: &[&Model], series: &[String]) -> Result<Vec<Columns>, Vec<Error>> {
    if series.is_empty() {
        let paths: std::collections::BTreeSet<&Path> = models
            .iter()
            .flat_map(|m| m.program.accounts.iter().map(AsRef::as_ref))
            .collect();
        return Ok(models
            .iter()
            .map(|model| {
                let columns = paths
                    .iter()
                    .map(|&path| Column {
                        name: path.ledger_name(),
                        of: Measure::Accounts(
                            model
                                .program
                                .accounts
                                .iter()
                                .position(|a| **a == *path)
                                .into_iter()
                                .collect(),
                            sign(path),
                        ),
                    })
                    .collect();
                (columns, Vec::new())
            })
            .collect());
    }
    let mut columns: Vec<Columns> = vec![(Vec::new(), Vec::new()); models.len()];
    let mut errors = Vec::new();
    for name in series {
        let measures: Vec<Option<Measure>> = models
            .iter()
            .zip(&mut columns)
            .map(|(model, (_, values))| model.measure(name, values))
            .collect();
        if measures.iter().all(Option::is_none) {
            errors.push(Error::UnknownSeries {
                name: name.clone(),
                close: models.first().and_then(|m| m.closest_series(name)),
            });
            continue;
        }
        let path = Path(name.split('.').map(str::to_string).collect());
        let missing = match path.account_kind() {
            Some(_) => Measure::Accounts(Vec::new(), sign(&path)),
            None => Measure::Missing,
        };
        for ((columns, _), measure) in columns.iter_mut().zip(measures) {
            columns.push(Column {
                name: name.clone(),
                of: measure.unwrap_or_else(|| missing.clone()),
            });
        }
    }
    if errors.is_empty() {
        Ok(columns)
    } else {
        Err(errors)
    }
}

/// `diags` as errors, each with a note naming the scenarios of `source` at
/// `applied`, since one may show up in the model, away from the scenario that
/// causes it. Errors in the scenarios themselves don't need it.
fn named_errors(source: &ast::Program, applied: &[usize], diags: Vec<Diagnostic>) -> Vec<Error> {
    let scenarios: Vec<&ast::Scenario> = applied.iter().map(|&i| &source.scenarios[i]).collect();
    diags
        .into_iter()
        .map(|diag| {
            if scenarios.iter().any(|s| scenario::contains(s, diag.span)) {
                return diag;
            }
            scenarios.iter().fold(diag, |diag, s| {
                diag.with_note(s.name.1, format!("with scenario `{}`", s.name.0))
            })
        })
        .map(Error::Diagnostic)
        .collect()
}

/// Models simulated side by side, like a model with several of its
/// scenarios: their CSV has the same columns, and each run draws with the
/// same seed in every one.
#[derive(Clone)]
pub struct SideBySide {
    /// Each model, and its name.
    models: Vec<(String, Model)>,
    /// What each one's CSV reports.
    columns: Vec<Columns>,
}

impl SideBySide {
    fn new(models: Vec<(String, Model)>, series: &[String]) -> Result<Self, Vec<Error>> {
        let refs: Vec<&Model> = models.iter().map(|(_, m)| m).collect();
        let columns = shared_columns(&refs, series)?;
        Ok(SideBySide { models, columns })
    }

    /// The models' names, in order.
    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.models.iter().map(|(name, _)| name.as_str())
    }

    /// Whether any of the models draws at random.
    pub fn draws(&self) -> bool {
        self.models.iter().any(|(_, m)| m.draws())
    }

    /// Simulates each model once, with the seed in `opts`. Their outputs
    /// don't keep transactions, since they're for CSV.
    pub fn simulate(&self, opts: &RunOpts) -> Result<Vec<Output>, Vec<Error>> {
        self.models
            .iter()
            .zip(&self.columns)
            .map(|((_, model), columns)| model.simulate_keeping(opts, false, columns))
            .collect()
    }

    /// Simulates runs 1 to `runs` of each model, each run drawing with its
    /// own seed (see [`Model::run_seed`]), the same in every model, on every
    /// core. Hands them to `each` in order of number, and of model within a
    /// number. Stops when `each` returns `false`.
    pub fn simulate_runs(&self, opts: &RunOpts, runs: u32, mut each: impl FnMut(Run) -> bool) {
        let per_run = self.models.len() as u64;
        let total = u64::from(runs) * per_run;
        let workers = std::thread::available_parallelism()
            .map_or(1, std::num::NonZero::get)
            .min(usize::try_from(total).unwrap_or(usize::MAX));
        let next = std::sync::atomic::AtomicU64::new(0);
        let (sender, receiver) = std::sync::mpsc::channel();
        std::thread::scope(|scope| {
            for _ in 0..workers {
                let sender = sender.clone();
                let next = &next;
                // Each worker has its own copy, whose caches it needn't
                // share.
                let models = self.clone();
                scope.spawn(move || {
                    loop {
                        let i = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        if i >= total {
                            break;
                        }
                        let number = u32::try_from(i / per_run + 1).expect("at most `runs`");
                        let scenario = (i % per_run) as usize;
                        let seed = Model::run_seed(opts.seed, number);
                        let opts = RunOpts {
                            seed,
                            ..opts.clone()
                        };
                        let result = models.models[scenario].1.simulate_keeping(
                            &opts,
                            false,
                            &models.columns[scenario],
                        );
                        let run = Run {
                            number,
                            seed,
                            scenario,
                            result,
                        };
                        // The receiver hangs up when `each` says to stop.
                        if sender.send((i, run)).is_err() {
                            break;
                        }
                    }
                });
            }
            drop(sender);
            // Runs finish in any order; hand them over in order.
            let mut waiting = std::collections::BTreeMap::new();
            let mut due = 0;
            for (i, run) in receiver {
                waiting.insert(i, run);
                while let Some(run) = waiting.remove(&due) {
                    due += 1;
                    if !each(run) {
                        return;
                    }
                }
            }
        });
    }
}

/// Renders errors from [`run`] for display, naming the model `path`. `color`
/// enables ANSI colors.
pub fn format_errors(path: &str, src: &str, errors: &[Error], color: bool) -> String {
    Sources::single(path, src).format_errors(errors, color)
}

/// Renders warnings from [`run`] for display, naming the model `path`. `color`
/// enables ANSI colors.
pub fn format_warnings(path: &str, src: &str, warnings: &[Diagnostic], color: bool) -> String {
    Sources::single(path, src).format_warnings(warnings, color)
}

fn emit_ledger(out: &mut impl io::Write, accounts: &[Path], log: &eval::SimLog) -> io::Result<()> {
    let start = log
        .snapshots
        .first()
        .map(|s| s.date)
        .unwrap_or_else(|| log.transactions.first().map(|t| t.date).unwrap_or_default());

    let mut opening: Vec<(String, Decimal)> = Vec::new();
    let mut equity = Decimal::ZERO;
    for path in accounts {
        let init = log.opening.get(path).copied().unwrap_or(Decimal::ZERO);
        equity -= init;
        if init != Decimal::ZERO {
            opening.push((path.ledger_name(), init));
        }
    }
    opening.push(("Equity:OpeningBalances".to_string(), equity));
    write_transaction(out, &format!("{start} opening-balances"), &opening)?;

    for tx in &log.transactions {
        let postings: Vec<(String, Decimal)> = tx
            .postings
            .iter()
            .map(|(account, amt)| (account.ledger_name(), *amt))
            .collect();
        write_transaction(out, &format!("{} {}", tx.date, tx.label), &postings)?;
    }
    Ok(())
}

/// Writes a ledger transaction with account names padded and amounts
/// right-aligned into columns.
fn write_transaction(
    out: &mut impl io::Write,
    header: &str,
    postings: &[(String, Decimal)],
) -> io::Result<()> {
    let amounts: Vec<String> = postings.iter().map(|(_, amt)| amt.to_string()).collect();
    // Format widths count characters, not bytes.
    let account_width = postings
        .iter()
        .map(|(a, _)| a.chars().count())
        .max()
        .unwrap_or(0);
    let amount_width = amounts.iter().map(String::len).max().unwrap_or(0);
    writeln!(out, "{header}")?;
    for ((account, _), amount) in postings.iter().zip(&amounts) {
        writeln!(out, "  {account:<account_width$}  {amount:>amount_width$}")?;
    }
    writeln!(out)
}

fn emit_csv(
    out: &mut impl io::Write,
    columns: &[Column],
    log: &eval::SimLog,
    run: Option<(u32, u64)>,
    scenario: Option<&str>,
    header: bool,
) -> io::Result<()> {
    if header {
        if run.is_some() {
            write!(out, "\"run\",\"seed\",")?;
        }
        if scenario.is_some() {
            write!(out, "\"scenario\",")?;
        }
        write!(out, "\"date\"")?;
        for column in columns {
            write!(out, ",\"{}\"", column.name)?;
        }
        writeln!(out)?;
    }

    for snap in &log.snapshots {
        if let Some((number, seed)) = run {
            write!(out, "{number},{seed},")?;
        }
        if let Some(scenario) = scenario {
            write!(out, "{scenario},")?;
        }
        write!(out, "{}", snap.date)?;
        for column in columns {
            match &column.of {
                Measure::Accounts(accounts, sign) => {
                    let v: Decimal = accounts.iter().map(|&i| snap.balances[i]).sum();
                    // Avoid printing -0.00.
                    let v = if v.is_zero() { Decimal::ZERO } else { v * sign };
                    write!(out, ",{v:.2}")?;
                }
                // Values aren't amounts, like an index of prices, so they
                // keep their digits.
                Measure::Value(i) => match snap.values[*i] {
                    Some(Value::Num(v)) => write!(out, ",{}", v.normalize())?,
                    Some(Value::Bool(b)) => write!(out, ",{b}")?,
                    None => write!(out, ",")?,
                },
                Measure::Missing => write!(out, ",")?,
            }
        }
        writeln!(out)?;
    }
    Ok(())
}

#[cfg(test)]
mod doc_tests {
    const DOCS: &[(&str, &str)] = &[
        ("README.md", include_str!("../README.md")),
        ("accounts.md", include_str!("../book/src/accounts.md")),
        ("checks.md", include_str!("../book/src/checks.md")),
        (
            "distributions.md",
            include_str!("../book/src/distributions.md"),
        ),
        ("monte-carlo.md", include_str!("../book/src/monte-carlo.md")),
        ("scenarios.md", include_str!("../book/src/scenarios.md")),
        ("entries.md", include_str!("../book/src/entries.md")),
        ("fns.md", include_str!("../book/src/fns.md")),
        ("intro.md", include_str!("../book/src/intro.md")),
        ("modules.md", include_str!("../book/src/modules.md")),
        ("schedules.md", include_str!("../book/src/schedules.md")),
        ("simulation.md", include_str!("../book/src/simulation.md")),
        ("values.md", include_str!("../book/src/values.md")),
    ];

    const KEYWORDS: &[&str] = &[
        "balances", "check", "entry", "fn", "let", "mod", "scenario", "drop",
    ];

    /// Fenced code blocks that look like saldo declarations, skipping syntax
    /// templates such as `let <name> = <expression>`.
    fn declaration_blocks(doc: &str) -> Vec<&str> {
        doc.split("```")
            .skip(1)
            .step_by(2)
            .filter(|block| {
                let first = block
                    .lines()
                    .map(str::trim)
                    .find(|line| !line.is_empty() && !line.starts_with("//"));
                let starts_with_keyword = first.is_some_and(|line| {
                    KEYWORDS.contains(&line.split_whitespace().next().unwrap_or(""))
                });
                starts_with_keyword && !has_placeholder(block)
            })
            .collect()
    }

    /// Whether `block` contains a `<placeholder>` like `<name>` or `<schedule-expression>`.
    fn has_placeholder(block: &str) -> bool {
        block.split('<').skip(1).any(|rest| {
            rest.split_once('>').is_some_and(|(word, _)| {
                !word.is_empty() && word.chars().all(|c| c.is_ascii_lowercase() || c == '-')
            })
        })
    }

    #[test]
    fn documented_examples_parse() {
        let mut failures = Vec::new();
        let mut checked = 0;
        for (name, doc) in DOCS {
            for block in declaration_blocks(doc) {
                checked += 1;
                // A scenario's file holds changes rather than declarations.
                let result = crate::lexer::lex(block, 0).and_then(|tokens| {
                    crate::parser::parse(tokens.clone())
                        .map(drop)
                        .or_else(|_| crate::parser::parse_changes(tokens).map(drop))
                });
                if let Err(diags) = result {
                    failures.push(format!("{name}:\n{block}\n{diags:?}"));
                }
            }
        }
        assert!(checked > 20, "only found {checked} examples");
        assert!(failures.is_empty(), "{}", failures.join("\n\n"));
    }
}
