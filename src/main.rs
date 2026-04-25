use chrono::NaiveDate;
use saldo::{OutputFormat, RunOpts, run};
use std::process::ExitCode;

const USAGE: &str = "usage: saldo <path> --from YYYY-MM-DD --to YYYY-MM-DD [--format ledger|csv]";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (path, from, to, format) = match parse_args(&args) {
        Ok(x) => x,
        Err(msg) => {
            eprintln!("{msg}\n{USAGE}");
            return ExitCode::from(2);
        }
    };

    let opts = RunOpts { from, to, format };
    match run(&path, &opts) {
        Ok(()) => ExitCode::SUCCESS,
        Err(()) => ExitCode::from(1),
    }
}

fn parse_args(
    args: &[String],
) -> Result<(String, NaiveDate, NaiveDate, OutputFormat), String> {
    let mut path: Option<String> = None;
    let mut from: Option<NaiveDate> = None;
    let mut to: Option<NaiveDate> = None;
    let mut format = OutputFormat::Ledger;
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
    Ok((
        path.ok_or("missing source path")?,
        from.ok_or("missing --from")?,
        to.ok_or("missing --to")?,
        format,
    ))
}

fn parse_date(s: &str) -> Result<NaiveDate, String> {
    NaiveDate::parse_from_str(s, "%Y-%m-%d").map_err(|e| format!("bad date `{s}`: {e}"))
}
