mod ast;
mod errors;
mod eval;
mod lexer;
mod parser;
mod resolve;

use chrono::NaiveDate;
use rust_decimal::Decimal;

pub use ast::Span;
pub use errors::Diagnostic;
pub use lexer::Lexer;
pub use parser::Parser;

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

    let tokens = match Lexer::new(&source).lex() {
        Ok(tokens) => tokens,
        Err(diagnostics) => {
            errors::report(path, &source, &diagnostics);
            return Err(());
        }
    };

    let program = match Parser::new(tokens).parse() {
        Ok(p) => p,
        Err(diags) => {
            errors::report(path, &source, &diags);
            return Err(());
        }
    };

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

    writeln!(out, "{start} opening-balances").ok();
    if let Some(first_snap) = log.snapshots.first() {
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

    for tx in &log.transactions {
        writeln!(out, "{} {}", tx.date, tx.label).ok();
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
