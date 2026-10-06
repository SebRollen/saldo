mod ast;
mod compile;
mod errors;
mod eval;
mod expand;
mod lexer;
mod parser;
mod random;
mod resolver;
mod sources;
mod typecheck;
mod units;
mod util;

use chrono::NaiveDate;
use rust_decimal::Decimal;
use std::io;

pub use ast::{Path, Span, TimeUnit};
pub use errors::{Diagnostic, Error};
pub use eval::{CheckFailure, DaySnapshot, SimLog, Transaction};
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
        emit_csv(out, &self.columns, &self.log, None, true)
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
        emit_csv(out, &self.columns, &self.log, Some((number, seed)), header)
    }
}

/// One of many runs: its number, from 1, the seed it drew with, and what it
/// did.
pub struct Run {
    pub number: u32,
    pub seed: u64,
    pub result: Result<Output, Vec<Error>>,
}

/// A model that's been read and checked, ready to simulate as often as wanted.
#[derive(Clone)]
pub struct Model {
    model: resolver::Model,
    program: compile::Program,
}

/// Reads and checks a model given as text. It can't have modules in other
/// files; use [`load_file`] for that.
pub fn load(src: &str) -> Result<Model, Vec<Error>> {
    let program = sources::load_text(&mut Sources::default(), src)?;
    Model::new(&program)
}

/// Reads and checks the model in the file at `path`, reading the files of its
/// modules. `sources` collects the text of every file read, for
/// [`Sources::format_errors`] and [`Sources::format_warnings`].
pub fn load_file(
    path: impl AsRef<std::path::Path>,
    sources: &mut Sources,
) -> Result<Model, Vec<Error>> {
    let program = sources::load_file(sources, path.as_ref())?;
    Model::new(&program)
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
    fn new(program: &ast::Program) -> Result<Self, Vec<Error>> {
        let model = resolver::resolve(program)
            .map_err(|diags| diags.into_iter().map(Error::Diagnostic).collect::<Vec<_>>())?;
        let mut program = compile::compile(&model);
        let diags = units::check(&mut program);
        if !diags.is_empty() {
            return Err(diags.into_iter().map(Error::Diagnostic).collect());
        }
        Ok(Model { model, program })
    }

    /// Whether the model draws at random, so that what it does depends on
    /// the seed.
    pub fn draws(&self) -> bool {
        !self.program.draws.is_empty()
    }

    /// Simulates the model as `opts` says.
    pub fn simulate(&self, opts: &RunOpts) -> Result<Output, Vec<Error>> {
        self.simulate_keeping(opts, true)
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
        let workers = std::thread::available_parallelism()
            .map_or(1, std::num::NonZero::get)
            .min(runs as usize);
        let next = std::sync::atomic::AtomicU32::new(1);
        let (sender, receiver) = std::sync::mpsc::channel();
        std::thread::scope(|scope| {
            for _ in 0..workers {
                let sender = sender.clone();
                let next = &next;
                // Each worker has its own copy, whose caches it needn't
                // share.
                let model = self.clone();
                scope.spawn(move || {
                    loop {
                        let number = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        if number > runs {
                            break;
                        }
                        let seed = Self::run_seed(opts.seed, number);
                        let opts = RunOpts {
                            seed,
                            ..opts.clone()
                        };
                        let result = model.simulate_keeping(&opts, false);
                        // The receiver hangs up when `each` says to stop.
                        if sender
                            .send(Run {
                                number,
                                seed,
                                result,
                            })
                            .is_err()
                        {
                            break;
                        }
                    }
                });
            }
            drop(sender);
            // Runs finish in any order; hand them over in order.
            let mut waiting = std::collections::BTreeMap::new();
            let mut due = 1;
            for run in receiver {
                waiting.insert(run.number, run);
                while let Some(run) = waiting.remove(&due) {
                    due += 1;
                    if !each(run) {
                        return;
                    }
                }
            }
        });
    }

    fn simulate_keeping(&self, opts: &RunOpts, transactions: bool) -> Result<Output, Vec<Error>> {
        check_range(opts)?;
        let (columns, values) = self.columns(&opts.series)?;
        let first_day = self.model.first_day(opts.from);
        let record = eval::Record {
            every: opts.every,
            values: &values,
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
                vec![Error::Diagnostic(d)]
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
            columns,
        })
    }

    /// The columns for `series`, or for every account if it's empty, and the
    /// values snapshots have to record for them.
    fn columns(&self, series: &[String]) -> Result<(Vec<Column>, Vec<usize>), Vec<Error>> {
        let accounts = &self.program.accounts;
        let sign = |path: &Path| path.account_kind().expect("accounts have kinds").sign();
        if series.is_empty() {
            let columns = accounts
                .iter()
                .enumerate()
                .map(|(i, path)| Column {
                    name: path.ledger_name(),
                    of: Measure::Accounts(vec![i], sign(path)),
                })
                .collect();
            return Ok((columns, Vec::new()));
        }
        let (mut columns, mut values, mut errors) = (Vec::new(), Vec::new(), Vec::new());
        for name in series {
            let path = Path(name.split('.').map(str::to_string).collect());
            let under: Vec<usize> = (0..accounts.len())
                .filter(|&i| accounts[i].starts_with(&path))
                .collect();
            let of = if path.account_kind().is_some() && !under.is_empty() {
                Measure::Accounts(under, sign(&path))
            } else if let Some(param) = self.program.params.iter().position(|p| p.name == *name) {
                values.push(param);
                Measure::Value(values.len() - 1)
            } else {
                let names: Vec<String> = accounts
                    .iter()
                    .map(|a| a.to_string())
                    .chain(self.program.params.iter().map(|p| p.name.clone()))
                    .collect();
                errors.push(Error::UnknownSeries {
                    name: name.clone(),
                    close: util::closest(name, names.iter().map(String::as_str))
                        .map(str::to_string),
                });
                continue;
            };
            columns.push(Column {
                name: name.clone(),
                of,
            });
        }
        if errors.is_empty() {
            Ok((columns, values))
        } else {
            Err(errors)
        }
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
    header: bool,
) -> io::Result<()> {
    if header {
        if run.is_some() {
            write!(out, "\"run\",\"seed\",")?;
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
                    Some(v) => write!(out, ",{}", v.normalize())?,
                    None => write!(out, ",")?,
                },
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
        ("entries.md", include_str!("../book/src/entries.md")),
        ("fns.md", include_str!("../book/src/fns.md")),
        ("intro.md", include_str!("../book/src/intro.md")),
        ("modules.md", include_str!("../book/src/modules.md")),
        ("schedules.md", include_str!("../book/src/schedules.md")),
        ("simulation.md", include_str!("../book/src/simulation.md")),
        ("values.md", include_str!("../book/src/values.md")),
    ];

    const KEYWORDS: &[&str] = &["balances", "check", "entry", "fn", "let", "mod"];

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
                let result = crate::lexer::lex(block, 0).and_then(crate::parser::parse);
                if let Err(diags) = result {
                    failures.push(format!("{name}:\n{block}\n{diags:?}"));
                }
            }
        }
        assert!(checked > 20, "only found {checked} examples");
        assert!(failures.is_empty(), "{}", failures.join("\n\n"));
    }
}
