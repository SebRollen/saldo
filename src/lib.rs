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

/// Renders errors for display. `color` enables ANSI colors.
pub fn format_errors(path: &str, src: &str, errors: &[Error], color: bool) -> String {
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
                    color,
                ));
            }
        }
    }
    out
}

fn emit_ledger(accounts: &[Path], log: &eval::SimLog) -> String {
    let mut out = String::new();

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
            opening.push((path.to_string(), init));
        }
    }
    opening.push(("Equity:OpeningBalances".to_string(), equity));
    write_transaction(&mut out, &format!("{start} opening-balances"), &opening);

    for tx in &log.transactions {
        let postings: Vec<(String, Decimal)> = tx
            .postings
            .iter()
            .map(|(account, amt)| (account.to_string(), *amt))
            .collect();
        write_transaction(&mut out, &format!("{} {}", tx.date, tx.label), &postings);
    }

    out
}

/// Writes a ledger transaction with account names padded and amounts
/// right-aligned into columns.
fn write_transaction(out: &mut String, header: &str, postings: &[(String, Decimal)]) {
    use std::fmt::Write;
    let amounts: Vec<String> = postings.iter().map(|(_, amt)| amt.to_string()).collect();
    let account_width = postings.iter().map(|(a, _)| a.len()).max().unwrap_or(0);
    let amount_width = amounts.iter().map(String::len).max().unwrap_or(0);
    writeln!(out, "{header}").ok();
    for ((account, _), amount) in postings.iter().zip(&amounts) {
        writeln!(out, "  {account:<account_width$}  {amount:>amount_width$}").ok();
    }
    writeln!(out).ok();
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

#[cfg(test)]
mod doc_tests {
    const DOCS: &[(&str, &str)] = &[
        ("README.md", include_str!("../README.md")),
        ("accounts.md", include_str!("../book/src/accounts.md")),
        ("asserts.md", include_str!("../book/src/asserts.md")),
        ("entries.md", include_str!("../book/src/entries.md")),
        ("fns.md", include_str!("../book/src/fns.md")),
        ("intro.md", include_str!("../book/src/intro.md")),
        ("params.md", include_str!("../book/src/params.md")),
        ("schedules.md", include_str!("../book/src/schedules.md")),
    ];

    const KEYWORDS: &[&str] = &["account", "assert", "entry", "fn", "param", "schedule"];

    /// Fenced code blocks that look like saldo declarations, skipping syntax
    /// templates such as `account <path>`.
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
                let result = crate::lexer::lex(block).and_then(crate::parser::parse);
                if let Err(diags) = result {
                    failures.push(format!("{name}:\n{block}\n{diags:?}"));
                }
            }
        }
        assert!(checked > 20, "only found {checked} examples");
        assert!(failures.is_empty(), "{}", failures.join("\n\n"));
    }
}
