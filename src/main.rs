use chrono::NaiveDate;
use saldo::{RunOpts, Sources, TimeUnit, run_file};
use std::hash::{BuildHasher, RandomState};
use std::io::{IsTerminal, Write};
use std::process::ExitCode;

const USAGE: &str = "usage: saldo <path> --from YYYY-MM-DD --to YYYY-MM-DD [--format ledger|csv] \
                     [--every UNIT] [--series NAME,...] [--seed N]";

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
  -h, --help          print this help
  -V, --version       print the version";

/// The exit status when the model ran but a check failed. Errors in the model
/// exit with 1, and bad arguments with 2.
const CHECK_FAILED: u8 = 3;

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
    match run_file(&path, &opts, &mut sources) {
        Ok(output) => {
            eprint!("{}", sources.format_warnings(&output.warnings, color));
            if output.draws && seed.is_none() {
                let seed = opts.seed;
                eprintln!("note: drew with seed {seed}; pass --seed {seed} to reproduce");
            }
            let mut out = std::io::BufWriter::new(std::io::stdout().lock());
            let written = match format {
                OutputFormat::Ledger => output.write_ledger(&mut out),
                OutputFormat::Csv => output.write_csv(&mut out),
            }
            .and_then(|()| out.flush());
            match written {
                Ok(()) if !output.log.failed_checks.is_empty() => {
                    let failed: Vec<saldo::Error> = output
                        .log
                        .failed_checks
                        .iter()
                        .map(|failure| saldo::Error::Diagnostic(failure.diagnostic()))
                        .collect();
                    eprint!("{}", sources.format_errors(&failed, color));
                    ExitCode::from(CHECK_FAILED)
                }
                Ok(()) => ExitCode::SUCCESS,
                // The reader went away (e.g. `saldo … | head`); that's not an error.
                Err(e) if e.kind() == std::io::ErrorKind::BrokenPipe => ExitCode::SUCCESS,
                Err(e) => {
                    eprintln!("could not write output: {e}");
                    ExitCode::from(1)
                }
            }
        }
        Err(errors) => {
            eprint!("{}", sources.format_errors(&errors, color));
            let bad_arguments = errors.iter().all(|e| {
                matches!(
                    e,
                    saldo::Error::InvalidDateRange { .. } | saldo::Error::UnknownSeries { .. }
                )
            });
            ExitCode::from(if bad_arguments { 2 } else { 1 })
        }
    }
}

struct Args {
    path: String,
    from: NaiveDate,
    to: NaiveDate,
    format: OutputFormat,
    seed: Option<u64>,
    every: TimeUnit,
    series: Vec<String>,
}

fn parse_args(args: &[String]) -> Result<Args, String> {
    let mut path: Option<String> = None;
    let mut from: Option<NaiveDate> = None;
    let mut to: Option<NaiveDate> = None;
    let mut format = OutputFormat::Ledger;
    let mut seed: Option<u64> = None;
    let mut every: Option<TimeUnit> = None;
    let mut series: Vec<String> = Vec::new();
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
                    Some("ledger") => OutputFormat::Ledger,
                    Some("csv") => OutputFormat::Csv,
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
    })
}

fn parse_date(s: &str) -> Result<NaiveDate, String> {
    NaiveDate::parse_from_str(s, "%Y-%m-%d").map_err(|e| format!("bad date `{s}`: {e}"))
}
