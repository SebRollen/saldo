use chrono::NaiveDate;
use saldo::{CheckFailure, Diagnostic, RunOpts, Sources, TimeUnit};
use std::hash::{BuildHasher, RandomState};
use std::io::{IsTerminal, Write};
use std::process::ExitCode;

const USAGE: &str = "usage: saldo <path> --from YYYY-MM-DD --to YYYY-MM-DD [--format ledger|csv] \
                     [--every UNIT] [--series NAME,...] [--seed N] [--runs N] [--scenario NAME]";

const HELP: &str = "\
Simulate a saldo model and print the resulting transactions or balances.

Options:
  --from YYYY-MM-DD   first day to report (inclusive)
  --to YYYY-MM-DD     last day to report (inclusive)
  --format FORMAT     `ledger` (default): double-entry transactions
                      `csv`: daily balance of every account
  --every UNIT        with csv, report the last day of each `week`, `month`,
                      `quarter` or `year` instead of every day
  --series NAME,...   with csv, report these accounts, groups of accounts
                      (summed) or values, like `assets.cash,assets,prices`,
                      instead of every account
  --seed N            draw the model's random numbers from seed N, a whole
                      number, to repeat a run (default: a random seed)
  --runs N            simulate N runs, each drawing with its own seed, and
                      write them all as CSV, with each run's number and seed;
                      `--seed` with a run's seed repeats it
  --scenario NAME     simulate the model with scenario NAME, or with the
                      scenarios NAME joins with `+`, like `prepay+daycare`
  -h, --help          print this help
  -V, --version       print the version";

/// The exit status when the model ran but a check failed. Errors in the model
/// exit with 1, and bad arguments with 2.
const CHECK_FAILED: u8 = 3;

#[derive(Clone, Copy)]
enum OutputFormat {
    Ledger,
    Csv,
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "-h" || a == "--help") {
        println!("{USAGE}\n\n{HELP}");
        return ExitCode::SUCCESS;
    }
    if args.iter().any(|a| a == "-V" || a == "--version") {
        println!("saldo {}", env!("CARGO_PKG_VERSION"));
        return ExitCode::SUCCESS;
    }
    let Args {
        path,
        from,
        to,
        format,
        seed,
        every,
        series,
        runs,
        scenario,
    } = match parse_args(&args) {
        Ok(x) => x,
        Err(msg) => {
            eprintln!("{msg}\n{USAGE}");
            return ExitCode::from(2);
        }
    };

    let opts = RunOpts {
        from,
        to,
        seed: seed.unwrap_or_else(|| RandomState::new().hash_one(())),
        every,
        series,
    };
    let color = std::io::stderr().is_terminal() && std::env::var_os("NO_COLOR").is_none();
    let mut sources = Sources::default();
    let model = match saldo::load_file(&path, &mut sources).and_then(|model| match &scenario {
        Some(name) => model.scenario(name),
        None => Ok(model),
    }) {
        Ok(model) => model,
        Err(errors) => return report_errors(&sources, &errors, color),
    };
    let report = Report {
        sources: &sources,
        color,
        random_seed: seed.is_none() && model.draws(),
    };
    match runs {
        None => report.one(&model, &opts, format),
        Some(runs) => report.many(&model, &opts, runs),
    }
}

/// How to report what a model did.
struct Report<'a> {
    sources: &'a Sources,
    color: bool,
    /// Whether the model draws with a seed picked at random, which the user
    /// needs to see to repeat the run.
    random_seed: bool,
}

impl Report<'_> {
    /// Simulates `model` once and writes `format` to stdout.
    fn one(&self, model: &saldo::Model, opts: &RunOpts, format: OutputFormat) -> ExitCode {
        let output = match model.simulate(opts) {
            Ok(output) => output,
            Err(errors) => return report_errors(self.sources, &errors, self.color),
        };
        eprint!(
            "{}",
            self.sources.format_warnings(&output.warnings, self.color)
        );
        self.note_seed(opts.seed);
        let mut out = std::io::BufWriter::new(std::io::stdout().lock());
        let written = match format {
            OutputFormat::Ledger => output.write_ledger(&mut out),
            OutputFormat::Csv => output.write_csv(&mut out),
        }
        .and_then(|()| out.flush());
        if let Err(code) = written_or_exit(written) {
            return code;
        }
        let failed: Vec<Diagnostic> = output
            .log
            .failed_checks
            .iter()
            .map(CheckFailure::diagnostic)
            .collect();
        self.report_failed_checks(failed)
    }

    /// Simulates `model` `runs` times and writes every run's CSV to stdout.
    fn many(&self, model: &saldo::Model, opts: &RunOpts, runs: u32) -> ExitCode {
        if runs > 1 && !model.draws() {
            eprintln!("note: the model doesn't draw at random, so every run is the same");
        }
        self.note_seed(opts.seed);
        let mut out = std::io::BufWriter::new(std::io::stdout().lock());
        let mut failures: Vec<RunsFailed> = Vec::new();
        let mut stopped: Option<ExitCode> = None;
        model.simulate_runs(opts, runs, |run| {
            let output = match run.result {
                Ok(output) => output,
                Err(errors) => {
                    stopped = Some(report_errors(self.sources, &errors, self.color));
                    return false;
                }
            };
            if run.number == 1 {
                eprint!(
                    "{}",
                    self.sources.format_warnings(&output.warnings, self.color)
                );
            }
            for failure in &output.log.failed_checks {
                match failures.iter_mut().find(|f| f.first.span == failure.span) {
                    Some(failed) => {
                        failed.runs += 1;
                        // Runs come in order, so a tie goes to the earlier.
                        if failure.first < failed.first.first {
                            *failed = RunsFailed {
                                first: failure.clone(),
                                number: run.number,
                                seed: run.seed,
                                runs: failed.runs,
                            };
                        }
                    }
                    None => failures.push(RunsFailed {
                        first: failure.clone(),
                        number: run.number,
                        seed: run.seed,
                        runs: 1,
                    }),
                }
            }
            match written_or_exit(output.write_run_csv(
                &mut out,
                run.number,
                run.seed,
                run.number == 1,
            )) {
                Ok(()) => true,
                Err(code) => {
                    stopped = Some(code);
                    false
                }
            }
        });
        if let Some(code) = stopped {
            return code;
        }
        if let Err(code) = written_or_exit(out.flush()) {
            return code;
        }
        failures.sort_by_key(|f| f.first.span.start);
        let failed = failures
            .iter()
            .map(|f| {
                let found = f
                    .first
                    .found
                    .as_ref()
                    .map_or(String::new(), |found| format!(": {found}"));
                Diagnostic::new(
                    f.first.span,
                    format!(
                        "check failed in {} of {runs} runs, first on {} in run {} (seed {}){found}",
                        f.runs, f.first.first, f.number, f.seed
                    ),
                )
            })
            .collect();
        self.report_failed_checks(failed)
    }

    fn note_seed(&self, seed: u64) {
        if self.random_seed {
            eprintln!("note: drew with seed {seed}; pass --seed {seed} to reproduce");
        }
    }

    /// Reports checks that failed, which the run (or runs) went on after, and
    /// exits with the status that says so.
    fn report_failed_checks(&self, failed: Vec<Diagnostic>) -> ExitCode {
        if failed.is_empty() {
            return ExitCode::SUCCESS;
        }
        let failed: Vec<saldo::Error> = failed.into_iter().map(saldo::Error::Diagnostic).collect();
        eprint!("{}", self.sources.format_errors(&failed, self.color));
        ExitCode::from(CHECK_FAILED)
    }
}

/// A check that failed in some of many runs: in how many, and the run where
/// it failed first.
struct RunsFailed {
    first: CheckFailure,
    number: u32,
    seed: u64,
    runs: u32,
}

/// `Err` with the status to exit with if writing failed. The reader going
/// away, as with `saldo … | head`, isn't an error.
fn written_or_exit(written: std::io::Result<()>) -> Result<(), ExitCode> {
    match written {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::BrokenPipe => Err(ExitCode::SUCCESS),
        Err(e) => {
            eprintln!("could not write output: {e}");
            Err(ExitCode::from(1))
        }
    }
}

/// Reports errors, exiting with 2 if they're all in the arguments and 1
/// otherwise.
fn report_errors(sources: &Sources, errors: &[saldo::Error], color: bool) -> ExitCode {
    eprint!("{}", sources.format_errors(errors, color));
    let bad_arguments = errors.iter().all(|e| {
        matches!(
            e,
            saldo::Error::InvalidDateRange { .. }
                | saldo::Error::UnknownSeries { .. }
                | saldo::Error::UnknownScenario { .. }
        )
    });
    ExitCode::from(if bad_arguments { 2 } else { 1 })
}

struct Args {
    path: String,
    from: NaiveDate,
    to: NaiveDate,
    format: OutputFormat,
    seed: Option<u64>,
    every: TimeUnit,
    series: Vec<String>,
    runs: Option<u32>,
    scenario: Option<String>,
}

fn parse_args(args: &[String]) -> Result<Args, String> {
    let mut path: Option<String> = None;
    let mut from: Option<NaiveDate> = None;
    let mut to: Option<NaiveDate> = None;
    let mut format: Option<OutputFormat> = None;
    let mut seed: Option<u64> = None;
    let mut runs: Option<u32> = None;
    let mut every: Option<TimeUnit> = None;
    let mut series: Vec<String> = Vec::new();
    let mut scenario: Option<String> = None;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--from" => {
                i += 1;
                from = Some(parse_date(args.get(i).ok_or("missing value for --from")?)?);
            }
            "--to" => {
                i += 1;
                to = Some(parse_date(args.get(i).ok_or("missing value for --to")?)?);
            }
            "--format" => {
                i += 1;
                format = match args.get(i).map(String::as_str) {
                    Some("ledger") => Some(OutputFormat::Ledger),
                    Some("csv") => Some(OutputFormat::Csv),
                    Some(other) => return Err(format!("unknown format `{other}`")),
                    None => return Err("missing value for --format".to_string()),
                };
            }
            "--seed" => {
                i += 1;
                let value = args.get(i).ok_or("missing value for --seed")?;
                seed = Some(value.parse().map_err(|_| {
                    format!(
                        "bad seed `{value}`: a seed is a whole number from 0 to {}",
                        u64::MAX
                    )
                })?);
            }
            "--runs" => {
                i += 1;
                let value = args.get(i).ok_or("missing value for --runs")?;
                runs =
                    Some(
                        value.parse().ok().filter(|&n: &u32| n > 0).ok_or_else(|| {
                            format!("bad --runs `{value}`: a whole number from 1")
                        })?,
                    );
            }
            "--every" => {
                i += 1;
                let value = args.get(i).ok_or("missing value for --every")?;
                every = Some(TimeUnit::from_name(value).ok_or_else(|| {
                    format!("bad --every `{value}`: `day`, `week`, `month`, `quarter` or `year`")
                })?);
            }
            "--series" => {
                i += 1;
                let value = args.get(i).ok_or("missing value for --series")?;
                series.extend(
                    value
                        .split(',')
                        .map(str::trim)
                        .filter(|name| !name.is_empty())
                        .map(str::to_string),
                );
            }
            "--scenario" => {
                i += 1;
                let value = args.get(i).ok_or("missing value for --scenario")?;
                scenario = Some(value.trim().to_string());
            }
            other if !other.starts_with("--") => {
                if path.is_some() {
                    return Err(format!("unexpected positional arg `{other}`"));
                }
                path = Some(other.to_string());
            }
            other => return Err(format!("unknown flag `{other}`")),
        }
        i += 1;
    }
    // Many runs write CSV; a run's ledger is its seed away.
    let format = match (format, runs) {
        (Some(OutputFormat::Ledger), Some(_)) => {
            return Err(
                "--runs writes CSV; for a run's ledger, pass its seed with --seed".to_string(),
            );
        }
        (Some(format), _) => format,
        (None, Some(_)) => OutputFormat::Csv,
        (None, None) => OutputFormat::Ledger,
    };
    if matches!(format, OutputFormat::Ledger) && (every.is_some() || !series.is_empty()) {
        return Err(
            "--every and --series pick what CSV reports; the ledger has every transaction"
                .to_string(),
        );
    }
    Ok(Args {
        path: path.ok_or("missing source path")?,
        from: from.ok_or("missing --from")?,
        to: to.ok_or("missing --to")?,
        format,
        seed,
        every: every.unwrap_or(TimeUnit::Day),
        series,
        runs,
        scenario,
    })
}

fn parse_date(s: &str) -> Result<NaiveDate, String> {
    NaiveDate::parse_from_str(s, "%Y-%m-%d").map_err(|e| format!("bad date `{s}`: {e}"))
}
