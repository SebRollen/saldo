use chrono::NaiveDate;
use rust_decimal::Decimal;
use saldo::{RunOpts, run};

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
    assert!(
        ledger
            .lines()
            .any(|line| line.split_whitespace().eq(["Assets:Cash", "1000"]))
    );
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
    let opening_cash = output
        .log
        .opening
        .get(&saldo::Path(vec!["Assets".to_string(), "Cash".to_string()]))
        .copied()
        .unwrap_or_default();
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
    assert!(
        errors
            .iter()
            .any(|e| matches!(e, saldo::Error::Diagnostic(d) if d.message.contains("opens on")))
    );
}

// --- user-defined functions ---

#[test]
fn fn_doubles_a_constant_param() {
    let src = "
        account Assets:Cash
        account Income:Salary

        fn double(x) { return x * 2; }
        param salary = 100
        param gross = double(salary)

        entry monthly \"pay\" {
          Assets:Cash = gross / 12
          Income:Salary
        }
    ";
    let output = run(src, &opts("2025-01-01", "2025-01-31")).unwrap();
    assert_eq!(output.log.transactions.len(), 1);
    let posting = output.log.transactions[0]
        .postings
        .iter()
        .find(|(p, _)| p.0 == vec!["Assets".to_string(), "Cash".to_string()])
        .unwrap();
    // gross = 200, gross/12 ≈ 16.67
    assert_eq!(posting.1.round_dp(2), Decimal::new(1667, 2));
}

#[test]
fn fn_with_let_binding() {
    let src = "
        account Assets:Cash
        account Expenses:Tax

        fn net(gross, rate) {
          let tax = gross * rate;
          return gross - tax;
        }

        entry monthly \"salary\" {
          Assets:Cash = net(6000, 0.3)
          Expenses:Tax
        }
    ";
    let output = run(src, &opts("2025-01-01", "2025-01-31")).unwrap();
    let posting = output.log.transactions[0]
        .postings
        .iter()
        .find(|(p, _)| p.0 == vec!["Assets".to_string(), "Cash".to_string()])
        .unwrap();
    // net(6000, 0.3) = 6000 - 1800 = 4200
    assert_eq!(posting.1, Decimal::new(4200, 0));
}

#[test]
fn fn_with_time_varying_param() {
    let src = "
        account Assets:Cash
        account Income:Salary

        fn double(x) { return x * 2; }

        param salary {
          from 2025-01-01 to 2025-07-01 = 100
          from 2025-07-01               = 200
        }
        param doubled = double(salary)

        entry monthly \"pay\" {
          Assets:Cash = doubled
          Income:Salary
        }
    ";
    let output = run(src, &opts("2025-01-01", "2025-12-31")).unwrap();
    assert_eq!(output.log.transactions.len(), 12);
    // January: doubled = 200
    let jan = &output.log.transactions[0];
    let jan_cash = jan
        .postings
        .iter()
        .find(|(p, _)| p.0 == vec!["Assets".to_string(), "Cash".to_string()])
        .unwrap();
    assert_eq!(jan_cash.1, Decimal::new(200, 0));
    // July: doubled = 400
    let jul = &output.log.transactions[6];
    let jul_cash = jul
        .postings
        .iter()
        .find(|(p, _)| p.0 == vec!["Assets".to_string(), "Cash".to_string()])
        .unwrap();
    assert_eq!(jul_cash.1, Decimal::new(400, 0));
}

#[test]
fn fn_calling_another_fn() {
    let src = "
        account Assets:Cash
        account Income:Salary

        fn double(x) { return x * 2; }
        fn quad(x)   { return double(double(x)); }

        entry monthly \"pay\" {
          Assets:Cash = quad(10)
          Income:Salary
        }
    ";
    let output = run(src, &opts("2025-01-01", "2025-01-31")).unwrap();
    let posting = output.log.transactions[0]
        .postings
        .iter()
        .find(|(p, _)| p.0 == vec!["Assets".to_string(), "Cash".to_string()])
        .unwrap();
    assert_eq!(posting.1, Decimal::new(40, 0));
}

#[test]
fn fn_calling_builtin() {
    let src = "
        account Assets:Cash
        account Income:Salary

        fn positive(x) { return max(x, 0); }

        entry monthly \"pay\" {
          Assets:Cash = positive(-50) + positive(30)
          Income:Salary
        }
    ";
    let output = run(src, &opts("2025-01-01", "2025-01-31")).unwrap();
    let posting = output.log.transactions[0]
        .postings
        .iter()
        .find(|(p, _)| p.0 == vec!["Assets".to_string(), "Cash".to_string()])
        .unwrap();
    // max(-50, 0) + max(30, 0)
    assert_eq!(posting.1, Decimal::new(30, 0));
}

#[test]
fn fn_with_if_expr() {
    let src = "
        account Assets:Cash
        account Income:Bonus

        fn bonus(salary, target) {
          return if target > 0 then salary * 0.1 else 0;
        }

        entry monthly \"bonus\" {
          Assets:Cash = bonus(10000, 1)
          Income:Bonus
        }
    ";
    let output = run(src, &opts("2025-01-01", "2025-01-31")).unwrap();
    let posting = output.log.transactions[0]
        .postings
        .iter()
        .find(|(p, _)| p.0 == vec!["Assets".to_string(), "Cash".to_string()])
        .unwrap();
    assert_eq!(posting.1, Decimal::new(1000, 0));
}

#[test]
fn fn_referencing_global_param_is_rejected() {
    let src = "
        param rate = 0.3
        fn bad(x) { return x * rate; }
    ";
    let errors = run(src, &opts("2025-01-01", "2025-01-01")).unwrap_err();
    assert!(errors.iter().any(
        |e| matches!(e, saldo::Error::Diagnostic(d) if d.message.contains("function bodies can only reference"))
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
    let opening_cash = output
        .log
        .opening
        .get(&saldo::Path(vec!["Assets".to_string(), "Cash".to_string()]))
        .copied()
        .unwrap_or_default();
    assert_eq!(opening_cash, rust_decimal::Decimal::new(500, 0));
}

#[test]
fn recursive_fn_is_rejected() {
    let src = "fn loop_(x) { return loop_(x); }";
    let errors = run(src, &opts("2025-01-01", "2025-01-01")).unwrap_err();
    assert!(errors.iter().any(
        |e| matches!(e, saldo::Error::Diagnostic(d) if d.message.contains("recursive call cycle"))
    ));
}

#[test]
fn duplicate_fn_name_is_rejected() {
    let src = "fn f(x) { return x; }\nfn f(y) { return y; }";
    let errors = run(src, &opts("2025-01-01", "2025-01-01")).unwrap_err();
    assert!(errors.iter().any(
        |e| matches!(e, saldo::Error::Diagnostic(d) if d.message.contains("duplicate function"))
    ));
}

#[test]
fn fn_implicit_return() {
    let src = "
        account Assets:Cash
        account Income:Salary

        fn double(x) { x * 2 }
        fn net(gross, rate) {
          let tax = gross * rate;
          gross - tax
        }

        entry monthly \"pay\" {
          Assets:Cash = net(double(50), 0.2)
          Income:Salary
        }
    ";
    let output = run(src, &opts("2025-01-01", "2025-01-31")).unwrap();
    let posting = output.log.transactions[0]
        .postings
        .iter()
        .find(|(p, _)| p.0 == vec!["Assets".to_string(), "Cash".to_string()])
        .unwrap();
    // double(50) = 100; net(100, 0.2) = 100 - 20 = 80
    assert_eq!(posting.1, Decimal::new(80, 0));
}

#[test]
fn fn_arity_mismatch_is_rejected() {
    let src = "
        account Assets:Cash
        account Income:Salary
        fn double(x) { return x * 2; }
        entry monthly \"pay\" {
          Assets:Cash = double(1, 2)
          Income:Salary
        }
    ";
    let errors = run(src, &opts("2025-01-01", "2025-01-01")).unwrap_err();
    assert!(
        errors
            .iter()
            .any(|e| matches!(e, saldo::Error::Diagnostic(d) if d.message.contains("argument")))
    );
}

fn has_error(errors: &[saldo::Error], needle: &str) -> bool {
    errors
        .iter()
        .any(|e| matches!(e, saldo::Error::Diagnostic(d) if d.message.contains(needle)))
}

// --- balancing ---

#[test]
fn unbalanced_explicit_postings_are_rejected() {
    let src = "
        account Assets:Cash
        account Income:Salary

        entry monthly \"Paycheck\" {
          Assets:Cash   = 500
          Income:Salary = -400
        }
    ";
    let errors = run(src, &opts("2025-01-01", "2025-01-31")).unwrap_err();
    assert!(has_error(&errors, "does not balance"));
}

#[test]
fn balanced_explicit_postings_are_accepted() {
    let src = "
        account Assets:Cash
        account Income:Salary

        entry monthly \"Paycheck\" {
          Assets:Cash   = 500
          Income:Salary = -500
        }
    ";
    run(src, &opts("2025-01-01", "2025-01-31")).unwrap();
}

#[test]
fn single_posting_entry_is_rejected() {
    let src = "
        account Assets:Cash
        entry monthly \"Free money\" { Assets:Cash = 500 }
    ";
    let errors = run(src, &opts("2025-01-01", "2025-01-31")).unwrap_err();
    assert!(has_error(&errors, "at least two postings"));
}

// --- params & opening balances ---

fn posting(output: &saldo::Output, tx: usize, account: &str) -> Decimal {
    output.log.transactions[tx]
        .postings
        .iter()
        .find(|(p, _)| p.to_string() == account)
        .map(|(_, v)| *v)
        .unwrap_or_else(|| panic!("no posting to {account} in transaction {tx}"))
}

#[test]
fn param_is_an_error_once_its_interval_ends() {
    let src = "
        account Assets:Cash
        account Income:Bonus
        param bonus { from 2025-01-01 to 2025-01-02 = 1000 }
        entry daily \"Bonus\" {
          Assets:Cash = bonus
          Income:Bonus
        }
    ";
    let errors = run(src, &opts("2025-01-01", "2025-01-03")).unwrap_err();
    assert!(has_error(
        &errors,
        "param `bonus` has no value on 2025-01-02"
    ));
}

#[test]
fn unused_param_without_a_value_is_not_an_error() {
    let src = "
        param bonus { from 2025-01-01 to 2025-01-02 = 1000 }
        param half = bonus / 2
    ";
    run(src, &opts("2025-01-01", "2025-01-03")).unwrap();
}

#[test]
fn opening_balance_can_reference_a_param() {
    let src = "
        param initial = 5_000
        account Assets:Cash = initial @ 2025-01-01
    ";
    let output = run(src, &opts("2025-01-01", "2025-01-01")).unwrap();
    let opening = output
        .log
        .opening
        .get(&saldo::Path(vec!["Assets".to_string(), "Cash".to_string()]));
    assert_eq!(opening, Some(&Decimal::new(5000, 0)));
}

#[test]
fn param_can_read_an_account_once_it_opens() {
    let src = "
        account Assets:Savings = 10 @ 2025-01-03
        account Assets:Cash
        account Income:Interest
        param doubled = Assets:Savings * 2
        entry 2025-01-03 \"Interest\" {
          Assets:Cash = doubled
          Income:Interest
        }
    ";
    let output = run(src, &opts("2025-01-01", "2025-01-03")).unwrap();
    let tx = output
        .log
        .transactions
        .iter()
        .position(|t| t.label == "Interest")
        .unwrap();
    assert_eq!(posting(&output, tx, "Assets:Cash"), Decimal::new(20, 0));
}

#[test]
fn account_opening_after_start_gets_an_opening_transaction() {
    let src = "account Assets:Cash = 1000 @ 2025-01-02";
    let output = run(src, &opts("2025-01-01", "2025-01-03")).unwrap();
    let tx = &output.log.transactions[0];
    assert_eq!(tx.date, d("2025-01-02"));
    assert_eq!(posting(&output, 0, "Assets:Cash"), Decimal::new(1000, 0));
    assert_eq!(
        posting(&output, 0, "Equity:OpeningBalances"),
        Decimal::new(-1000, 0)
    );
}

// --- hostile input ---

#[test]
fn arithmetic_overflow_is_a_diagnostic() {
    let src = "
        account Assets:Cash = 1 @ 2025-01-01
        account Income:Magic
        entry daily \"Grow\" {
          Assets:Cash = Assets:Cash * 1_000_000
          Income:Magic
        }
    ";
    let errors = run(src, &opts("2025-01-01", "2025-12-31")).unwrap_err();
    assert!(has_error(&errors, "overflow"));
}

#[test]
fn non_ascii_outside_a_string_is_a_diagnostic() {
    for src in ["account Café", "param p = 1sé"] {
        let errors = run(src, &opts("2025-01-01", "2025-01-01")).unwrap_err();
        assert!(has_error(&errors, "unexpected character `é`"), "{src}");
        // Rendering must not split the multi-byte character.
        saldo::format_errors("test.saldo", src, &errors, true);
    }
}

#[test]
fn deeply_nested_expressions_are_rejected() {
    let parens = format!("param p = {}1{}", "(".repeat(20_000), ")".repeat(20_000));
    let negations = format!("param p = {}1", "-".repeat(20_000));
    let chain = format!("param p = {}", vec!["1"; 20_000].join(" + "));
    for src in [parens, negations, chain] {
        let errors = run(&src, &opts("2025-01-01", "2025-01-01")).unwrap_err();
        assert!(has_error(&errors, "nested too deeply"));
    }
}

#[test]
fn expressions_near_the_nesting_limit_evaluate() {
    let src = format!(
        "param p = {}1{}\nparam q = {}\nassert that p + q == 251",
        "(".repeat(250),
        ")".repeat(250),
        vec!["1"; 250].join(" + "),
    );
    run(&src, &opts("2025-01-01", "2025-01-01")).unwrap();
}

// --- name resolution ---

#[test]
fn invalid_date_is_a_diagnostic() {
    let errors = run("param p = 2025-13-01", &opts("2025-01-01", "2025-01-01")).unwrap_err();
    assert!(has_error(&errors, "invalid date `2025-13-01`"));
}

#[test]
fn fn_named_like_a_builtin_is_rejected() {
    let errors = run("fn min(a, b) { a + b }", &opts("2025-01-01", "2025-01-01")).unwrap_err();
    assert!(has_error(&errors, "same name as a built-in"));
}

#[test]
fn param_cycles_are_reported_once_with_their_path() {
    let src = "
        param a = b
        param b = c + 1
        param c = a
        param d = a
        param e = e * 2
    ";
    let errors = run(src, &opts("2025-01-01", "2025-01-01")).unwrap_err();
    assert_eq!(errors.len(), 2, "{errors:?}");
    assert!(has_error(
        &errors,
        "param `a` depends on itself (a → b → c → a)"
    ));
    assert!(has_error(&errors, "param `e` depends on itself (e → e)"));
}

#[test]
fn leg_name_conflicting_with_a_later_param_is_rejected() {
    let src = "
        account Assets:Cash
        account Income:Salary
        entry monthly \"Paycheck\" {
          Assets:Cash = 10 as rate
          Income:Salary
        }
        param rate = 0.5
    ";
    let errors = run(src, &opts("2025-01-01", "2025-01-31")).unwrap_err();
    assert!(has_error(&errors, "leg name `rate` conflicts with a param"));
}

#[test]
fn leg_name_conflicting_with_an_account_is_rejected() {
    let src = "
        account Cash
        account Income
        entry monthly \"Paycheck\" {
          Cash = 10 as Income
          Income
        }
    ";
    let errors = run(src, &opts("2025-01-01", "2025-01-31")).unwrap_err();
    assert!(has_error(
        &errors,
        "leg name `Income` conflicts with an account"
    ));
}

#[test]
fn referencing_the_auto_balanced_leg_is_rejected() {
    let src = "
        account Assets:Cash
        account Expenses:Tax
        account Income:Salary
        entry monthly \"Paycheck\" {
          Assets:Cash  = 100 + gross
          Expenses:Tax = 5
          Income:Salary as gross
        }
    ";
    let errors = run(src, &opts("2025-01-01", "2025-01-31")).unwrap_err();
    assert!(has_error(&errors, "`gross` is the auto-balanced leg"));
}

// --- amounts ---

#[test]
fn clearing_an_empty_account_posts_zero_not_negative_zero() {
    let src = "
        account Liabilities:Accrued
        account Assets:Cash
        entry daily \"Clear\" {
          Liabilities:Accrued = all
          Assets:Cash
        }
    ";
    let ledger = run(src, &opts("2025-01-01", "2025-01-01"))
        .unwrap()
        .to_ledger();
    assert!(
        !ledger.lines().any(|line| line.ends_with(" -0")),
        "{ledger}"
    );
}

#[test]
fn entries_that_move_no_money_are_skipped() {
    let src = "
        account Liabilities:Accrued = 50 @ 2025-01-01
        account Assets:Cash
        entry daily \"Clear\" {
          Liabilities:Accrued = all
          Assets:Cash
        }
    ";
    let output = run(src, &opts("2025-01-01", "2025-01-03")).unwrap();
    // Only the first day has a balance to clear.
    assert_eq!(output.log.transactions.len(), 1);
    assert_eq!(output.log.transactions[0].date, d("2025-01-01"));
}

#[test]
fn opening_balances_are_rounded_like_postings() {
    let src = "
        account Liabilities:Accrued = 100 / 3 @ 2025-01-01
        account Assets:Cash
        entry daily \"Clear\" {
          Liabilities:Accrued = all
          Assets:Cash
        }
        assert that Liabilities:Accrued == 0
    ";
    let output = run(src, &opts("2025-01-01", "2025-01-01")).unwrap();
    assert_eq!(
        posting(&output, 0, "Liabilities:Accrued"),
        Decimal::new(-3333, 2)
    );
}

// --- diagnostic spans ---

/// The source text highlighted by the error whose message contains `needle`.
fn highlighted<'a>(src: &'a str, errors: &[saldo::Error], needle: &str) -> &'a str {
    let span = errors
        .iter()
        .find_map(|e| match e {
            saldo::Error::Diagnostic(d) if d.message.contains(needle) => Some(d.span),
            _ => None,
        })
        .unwrap_or_else(|| panic!("no error containing {needle:?} in {errors:?}"));
    &src[span.start..span.end]
}

#[test]
fn resolver_errors_point_at_the_offending_text() {
    let cases = [
        (
            "account A\nentry daily \"x\" {\n A = 1\n Nowhere:Else\n}",
            "unknown account",
            "Nowhere:Else",
        ),
        (
            "account A\naccount B\nentry daily \"x\" {\n A = 1 as leg\n B as leg\n}",
            "duplicate leg name",
            "leg",
        ),
        (
            "account A\naccount B\naccount C\nentry daily \"x\" {\n A = 1\n B\n C\n}",
            "only one posting",
            "C",
        ),
        (
            "param p {\n from 2025-01-01 = 1\n from 2025-06-01 = 2\n}",
            "overlapping intervals",
            "from 2025-06-01 = 2",
        ),
        (
            "param p {\n from 2025-06-01 to 2025-01-01 = 1\n}",
            "must end after it starts",
            "from 2025-06-01 to 2025-01-01 = 1",
        ),
        (
            "account A\naccount B\nentry payday \"x\" {\n A = 1\n B\n}",
            "not defined",
            "payday",
        ),
        ("fn f(x, y, x) { x }", "duplicate parameter", "x"),
        ("fn f(x) { x.ytd }", "cannot use", "x.ytd"),
    ];
    for (src, needle, expected) in cases {
        let errors = run(src, &opts("2025-01-01", "2025-01-01")).unwrap_err();
        assert_eq!(highlighted(src, &errors, needle), expected, "{src}");
    }
}

#[test]
fn named_schedule_missing_from_is_reported_once() {
    let src = "
        schedule biweekly = every 2 weeks
        account A
        account B
        entry biweekly \"x\" { A = 1
          B }
        assert biweekly that A >= 0
    ";
    let errors = run(src, &opts("2025-01-01", "2025-01-01")).unwrap_err();
    assert_eq!(errors.len(), 1, "{errors:?}");
}

#[test]
fn errors_render_without_color_when_asked() {
    let src = "param p = ghost";
    let errors = run(src, &opts("2025-01-01", "2025-01-01")).unwrap_err();
    let plain = saldo::format_errors("test.saldo", src, &errors, false);
    assert!(!plain.contains('\x1b'), "{plain}");
    assert!(saldo::format_errors("test.saldo", src, &errors, true).contains('\x1b'));
}

// --- invariants ---

/// A model exercising most features: warm-up, late openings, params with
/// intervals, named legs and aggregates, `all`, functions and schedules.
const KITCHEN_SINK: &str = "
    account Assets:Cash                 =   5_000 @ 2025-01-01
    account Assets:Retirement           =  10_000 @ 2025-01-01
    account Assets:Savings              =   2_500 @ 2026-03-01
    account Liabilities:Loan            = -30_000 @ 2025-01-01
    account Liabilities:AccruedInterest
    account Income:Salary
    account Expenses:Interest
    account Expenses:Rent

    schedule payday = monthly on the 15th and last day
    param interest_rate = 0.05
    param max_401k = 23_000
    param salary {
        from 2025-01-01 to 2026-04-16 = 80_000
        from 2026-04-16               = 95_000
    }

    fn per_paycheck(annual) { annual / 24 }

    entry payday \"Paycheck\" {
      Assets:Retirement = min(max_401k - k401.ytd, per_paycheck(salary) * 0.15) as k401
      Assets:Cash       = per_paycheck(salary) - k401
      Income:Salary
    } as paycheck

    entry daily \"Interest accrual\" {
      Liabilities:AccruedInterest = Liabilities:Loan * interest_rate / 365
      Expenses:Interest
    }

    entry monthly on the 17th \"Loan payment\" {
      Liabilities:AccruedInterest = all
      Liabilities:Loan            = 1_000
      Assets:Cash
    }

    entry every 2 weeks on friday from 2025-01-03 \"Rent\" {
      Expenses:Rent = 1_200
      Assets:Cash
    }

    assert that paycheck.k401.ytd <= max_401k
";

#[test]
fn every_transaction_balances() {
    let output = run(KITCHEN_SINK, &opts("2026-01-01", "2026-12-31")).unwrap();
    assert!(output.log.transactions.len() > 300);
    for tx in &output.log.transactions {
        let sum: Decimal = tx.postings.iter().map(|(_, amt)| amt).sum();
        assert!(sum.is_zero(), "{} {} sums to {sum}", tx.date, tx.label);
    }
}

#[test]
fn ledger_postings_reproduce_the_daily_balances() {
    let output = run(KITCHEN_SINK, &opts("2026-01-01", "2026-12-31")).unwrap();
    let mut balances = output.log.opening.clone();
    let mut snapshots = output.log.snapshots.iter().peekable();
    for tx in &output.log.transactions {
        // Before applying a day's transactions, every earlier day must match.
        while let Some(snap) = snapshots.next_if(|s| s.date < tx.date) {
            assert_eq!(balances, snap.balances, "balances differ on {}", snap.date);
        }
        for (account, amt) in &tx.postings {
            if let Some(balance) = balances.get_mut(account) {
                *balance += amt;
            }
        }
    }
    for snap in snapshots {
        assert_eq!(balances, snap.balances, "balances differ on {}", snap.date);
    }
}
