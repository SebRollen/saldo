mod ast;
mod errors;
mod eval;
mod lexer;
mod parser;
mod resolver;
mod util;

use chrono::NaiveDate;
use rust_decimal::Decimal;

pub use ast::{Path, Span};
pub use errors::{Diagnostic, Error};
pub use eval::{DaySnapshot, SimLog, Transaction};

pub struct RunOpts {
    pub from: NaiveDate,
    pub to: NaiveDate,
}

#[derive(Debug)]
pub struct Output {
    pub accounts: Vec<Path>,
    pub log: SimLog,
}

impl Output {
    pub fn to_ledger(&self) -> String {
        emit_ledger(&self.accounts, &self.log)
    }

    pub fn to_csv(&self) -> String {
        emit_csv(&self.accounts, &self.log)
    }
}

pub fn run(src: &str, opts: &RunOpts) -> Result<Output, Vec<Error>> {
    if opts.from > opts.to {
        return Err(vec![Error::InvalidDateRange {
            from: opts.from,
            to: opts.to,
        }]);
    }

    let tokens = lexer::lex(src)
        .map_err(|diags| diags.into_iter().map(Error::Diagnostic).collect::<Vec<_>>())?;

    let program = parser::parse(tokens)
        .map_err(|diags| diags.into_iter().map(Error::Diagnostic).collect::<Vec<_>>())?;

    let model = resolver::resolve(&program)
        .map_err(|diags| diags.into_iter().map(Error::Diagnostic).collect::<Vec<_>>())?;

    let log = model
        .simulate(opts.from, opts.to)
        .map_err(|d| vec![Error::Diagnostic(d)])?;

    let accounts = model.stocks.keys().cloned().collect();

    Ok(Output { accounts, log })
}

pub fn format_errors(path: &str, src: &str, errors: &[Error]) -> String {
    use std::fmt::Write;
    let mut out = String::new();
    for e in errors {
        match e {
            Error::InvalidDateRange { from, to } => {
                writeln!(out, "--from ({from}) is after --to ({to})").ok();
            }
            Error::Diagnostic(d) => {
                out.push_str(&errors::format_diagnostics(
                    path,
                    src,
                    std::slice::from_ref(d),
                ));
            }
        }
    }
    out
}

fn emit_ledger(accounts: &[Path], log: &eval::SimLog) -> String {
    use std::fmt::Write;
    let mut out = String::new();

    let start = log
        .snapshots
        .first()
        .map(|s| s.date)
        .unwrap_or_else(|| log.transactions.first().map(|t| t.date).unwrap_or_default());

    writeln!(out, "{start} opening-balances").ok();
    let mut equity = Decimal::ZERO;
    for path in accounts {
        let init = log.opening.get(path).copied().unwrap_or(Decimal::ZERO);
        equity -= init;
        if init != Decimal::ZERO {
            writeln!(out, "  {path}  {}", init).ok();
        }
    }
    writeln!(out, "  Equity:OpeningBalances  {}", equity).ok();
    writeln!(out).ok();

    for tx in &log.transactions {
        writeln!(out, "{} {}", tx.date, tx.label).ok();
        for (account, amt) in &tx.postings {
            writeln!(out, "  {account}  {}", amt).ok();
        }
        writeln!(out).ok();
    }

    out
}

fn emit_csv(accounts: &[Path], log: &eval::SimLog) -> String {
    use std::fmt::Write;
    let mut out = String::new();

    write!(out, "\"date\"").ok();
    for name in accounts {
        write!(out, ",\"{name}\"").ok();
    }
    writeln!(out).ok();

    for snap in &log.snapshots {
        write!(out, "{}", snap.date).ok();
        for name in accounts {
            let v = snap.balances.get(name).copied().unwrap_or(Decimal::ZERO);
            write!(out, ",{v:.2}").ok();
        }
        writeln!(out).ok();
    }

    out
}
