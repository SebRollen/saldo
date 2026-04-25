mod ast;
mod errors;
mod eval;
mod layout;
mod lexer;
mod parser;
mod resolve;

use chrono::NaiveDate;
use chumsky::input::Input;
use chumsky::prelude::*;
use rust_decimal::Decimal;

pub use lexer::lexer;

pub enum OutputFormat {
    Ledger,
    Csv,
}

pub struct RunOpts {
    pub from: NaiveDate,
    pub to: NaiveDate,
    pub format: OutputFormat,
}

pub fn run(path: &str, opts: &RunOpts) -> Result<(), ()> {
    let source = match std::fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("could not read `{path}`: {e}");
            return Err(());
        }
    };
    if opts.from > opts.to {
        eprintln!("--from ({}) is after --to ({})", opts.from, opts.to);
        return Err(());
    }

    let (tokens, lex_errs) = lexer().parse(&source).into_output_errors();
    let lex_diags: Vec<_> = lex_errs
        .into_iter()
        .map(errors::Diagnostic::from_lex_err)
        .collect();
    if !lex_diags.is_empty() {
        errors::report(path, &source, &lex_diags);
        return Err(());
    }
    let tokens = layout::layout(tokens.unwrap());

    let eoi = (source.len()..source.len()).into();
    let input = tokens.as_slice().map(eoi, |(t, s)| (t, s));
    let (program, parse_errs) = parser::parser().parse(input).into_output_errors();
    let parse_diags: Vec<_> = parse_errs
        .into_iter()
        .map(errors::Diagnostic::from_parse_err)
        .collect();
    if !parse_diags.is_empty() {
        errors::report(path, &source, &parse_diags);
        return Err(());
    }
    let program = program.unwrap();

    let model = match resolve::resolve(&program) {
        Ok(m) => m,
        Err(diags) => {
            errors::report(path, &source, &diags);
            return Err(());
        }
    };

    let log = match model.simulate(opts.from, opts.to) {
        Ok(l) => l,
        Err(d) => {
            errors::report(path, &source, &[d]);
            return Err(());
        }
    };

    match opts.format {
        OutputFormat::Ledger => emit_ledger(&model, &log, opts.from),
        OutputFormat::Csv => emit_csv(&model, &log),
    }

    Ok(())
}

fn emit_ledger(model: &resolve::Model, log: &eval::SimLog, start: NaiveDate) {
    use std::io::{self, Write};
    let mut out = io::stdout().lock();

    // Opening-balances transaction: balances before any flows ran on `start`.
    writeln!(out, "{start} opening-balances").ok();
    if let Some(first_snap) = log.snapshots.first() {
        // Back out any flows that fired on `start` to recover the initial state.
        let mut equity = Decimal::ZERO;
        for path in model.stocks.keys() {
            let snap_bal = first_snap
                .balances
                .get(path)
                .copied()
                .unwrap_or(Decimal::ZERO);
            let flow_delta: Decimal = log
                .transactions
                .iter()
                .filter(|tx| tx.date == start)
                .flat_map(|tx| tx.postings.iter())
                .filter(|(acc, _)| acc == path)
                .map(|(_, amt)| *amt)
                .sum();
            let init = snap_bal - flow_delta;
            equity -= init;
            if init != Decimal::ZERO {
                writeln!(out, "  {path}  {}", init).ok();
            }
        }
        writeln!(out, "  Equity:OpeningBalances  {}", equity).ok();
    }
    writeln!(out).ok();

    // One ledger transaction per flow firing.
    for tx in &log.transactions {
        writeln!(out, "{} {}", tx.date, tx.flow).ok();
        for (account, amt) in &tx.postings {
            writeln!(out, "  {account}  {}", amt).ok();
        }
        writeln!(out).ok();
    }
}

fn emit_csv(model: &resolve::Model, log: &eval::SimLog) {
    use std::io::{self, Write};
    let mut out = io::stdout().lock();

    write!(out, "date").ok();
    for name in model.stocks.keys() {
        write!(out, ",{name}").ok();
    }
    writeln!(out).ok();

    for snap in &log.snapshots {
        write!(out, "{}", snap.date).ok();
        for name in model.stocks.keys() {
            let v = snap.balances.get(name).copied().unwrap_or(Decimal::ZERO);
            write!(out, ",{v:.2}").ok();
        }
        writeln!(out).ok();
    }
}
