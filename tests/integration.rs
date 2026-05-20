use chrono::NaiveDate;
use saldo::{run, RunOpts};

fn d(s: &str) -> NaiveDate {
    NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap()
}

fn opts(from: &str, to: &str) -> RunOpts {
    RunOpts {
        from: d(from),
        to: d(to),
    }
}

// --- happy path ---

#[test]
fn ledger_output_contains_transactions() {
    let src = "
        account Assets:Cash = 1000 @ 2025-01-01
        account Income:Salary

        entry monthly \"Paycheck\" {
          Assets:Cash = 500
          Income:Salary
        }
    ";
    let output = run(src, &opts("2025-01-01", "2025-03-31")).unwrap();
    let ledger = output.to_ledger();
    assert!(ledger.contains("opening-balances"));
    assert!(ledger.contains("Assets:Cash  1000"));
    assert!(ledger.contains("Paycheck"));
}

#[test]
fn csv_output_has_header_and_rows() {
    let src = "
        account Assets:Cash = 200 @ 2025-01-01
        account Liabilities:Loan = -500 @ 2025-01-01
    ";
    let output = run(src, &opts("2025-01-01", "2025-01-03")).unwrap();
    let csv = output.to_csv();
    let lines: Vec<&str> = csv.lines().collect();
    assert_eq!(lines[0], r#""date","Assets:Cash","Liabilities:Loan""#);
    assert_eq!(lines.len(), 4); // header + 3 days
}

#[test]
fn single_day_range_is_accepted() {
    let output = run(
        "account Assets:Cash = 100 @ 2025-06-01",
        &opts("2025-06-01", "2025-06-01"),
    )
    .unwrap();
    assert!(output.to_ledger().contains("opening-balances"));
}

#[test]
fn log_is_accessible_directly() {
    let src = "
        account Assets:Cash = 1000 @ 2025-01-01
        account Income:Salary

        entry monthly \"Paycheck\" {
          Assets:Cash = 500
          Income:Salary
        }
    ";
    let output = run(src, &opts("2025-01-01", "2025-01-31")).unwrap();
    assert_eq!(output.log.snapshots.len(), 31);
    assert_eq!(output.log.transactions.len(), 1);
    assert_eq!(output.log.transactions[0].label, "Paycheck");
}

// --- option validation ---

#[test]
fn from_after_to_returns_error() {
    let errors = run("account Assets:Cash", &opts("2025-12-31", "2025-01-01")).unwrap_err();
    assert!(matches!(errors[0], saldo::Error::InvalidDateRange { .. }));
}

// --- lex / parse errors ---

#[test]
fn lexer_error_is_a_diagnostic() {
    let errors = run(
        "account Assets:Cash\n####",
        &opts("2025-01-01", "2025-01-31"),
    )
    .unwrap_err();
    assert!(matches!(errors[0], saldo::Error::Diagnostic(_)));
}

#[test]
fn parse_error_is_a_diagnostic() {
    let src = "
        account Assets:Cash
        account Income:Salary

        entry monthly \"Broken\" {
          Assets:Cash = 100
    ";
    let errors = run(src, &opts("2025-01-01", "2025-01-31")).unwrap_err();
    assert!(matches!(errors[0], saldo::Error::Diagnostic(_)));
}

// --- resolve errors ---

#[test]
fn unknown_account_in_posting_is_a_diagnostic() {
    let src = "
        account Assets:Cash

        entry monthly \"Paycheck\" {
          Assets:Cash    = 500
          Income:Nowhere
        }
    ";
    let errors = run(src, &opts("2025-01-01", "2025-01-31")).unwrap_err();
    assert!(errors.iter().any(
        |e| matches!(e, saldo::Error::Diagnostic(d) if d.message.contains("unknown account"))
    ));
}

#[test]
fn unknown_param_in_expr_is_a_diagnostic() {
    let src = "
        account Assets:Cash
        account Income:Salary

        entry monthly \"Paycheck\" {
          Assets:Cash = ghost_param
          Income:Salary
        }
    ";
    let errors = run(src, &opts("2025-01-01", "2025-01-31")).unwrap_err();
    assert!(errors.iter().any(
        |e| matches!(e, saldo::Error::Diagnostic(d) if d.message.contains("unknown reference"))
    ));
}

// --- runtime / assertion errors ---

#[test]
fn failing_assertion_is_a_diagnostic() {
    let src = "
        account Assets:Cash = 100 @ 2025-01-01
        assert that Assets:Cash >= 200
    ";
    let errors = run(src, &opts("2025-01-01", "2025-01-01")).unwrap_err();
    assert!(errors.iter().any(
        |e| matches!(e, saldo::Error::Diagnostic(d) if d.message.contains("assertion failed"))
    ));
}

#[test]
fn passing_assertion_succeeds() {
    let src = "
        account Assets:Cash = 500 @ 2025-01-01
        assert that Assets:Cash >= 0
    ";
    run(src, &opts("2025-01-01", "2025-01-31")).unwrap();
}

#[test]
fn opening_balance_before_sim_start_warms_up() {
    let src = "
        account Assets:Cash = 1000 @ 2024-01-01
        account Income:Salary

        entry monthly \"Paycheck\" {
          Assets:Cash = 500
          Income:Salary
        }
    ";
    // Simulate from 2025-01-01, with opening on 2024-01-01.
    // The warm-up should apply 12 monthly entries, so opening at 2025-01-01 is 1000 + 12*500 = 7000.
    let output = run(src, &opts("2025-01-01", "2025-01-01")).unwrap();
    let opening_cash = output.log.opening.get(&saldo::Path(vec!["Assets".to_string(), "Cash".to_string()])).copied().unwrap_or_default();
    assert_eq!(opening_cash, rust_decimal::Decimal::new(7000, 0));
}

#[test]
fn reference_before_opening_date_is_error() {
    let src = "
        account Assets:Cash = 1000 @ 2025-06-01
        account Income:Salary

        entry monthly \"Paycheck\" {
          Assets:Cash = 500
          Income:Salary
        }
    ";
    // Simulation starts 2025-01-01, before Assets:Cash opens on 2025-06-01.
    // The entry fires in January and references Assets:Cash before it opens.
    let errors = run(src, &opts("2025-01-01", "2025-01-31")).unwrap_err();
    assert!(errors.iter().any(
        |e| matches!(e, saldo::Error::Diagnostic(d) if d.message.contains("opens on"))
    ));
}

#[test]
fn opening_balance_date_equals_sim_start() {
    let src = "
        account Assets:Cash = 500 @ 2025-03-01
        account Income:Salary

        entry monthly \"Paycheck\" {
          Assets:Cash = 100
          Income:Salary
        }
    ";
    let output = run(src, &opts("2025-03-01", "2025-03-31")).unwrap();
    let opening_cash = output.log.opening.get(&saldo::Path(vec!["Assets".to_string(), "Cash".to_string()])).copied().unwrap_or_default();
    assert_eq!(opening_cash, rust_decimal::Decimal::new(500, 0));
}
