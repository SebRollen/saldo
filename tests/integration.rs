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
    assert_eq!(&*output.log.transactions[0].label, "Paycheck");
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
        .position(|t| &*t.label == "Interest")
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
    for src in ["account Caf€", "param p = 1s€"] {
        let errors = run(src, &opts("2025-01-01", "2025-01-01")).unwrap_err();
        assert!(has_error(&errors, "unexpected character `€`"), "{src}");
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
    // Snapshot balances are in the order of `output.accounts`.
    let mut balances: Vec<Decimal> = output
        .accounts
        .iter()
        .map(|a| output.log.opening[a])
        .collect();
    let mut snapshots = output.log.snapshots.iter().peekable();
    for tx in &output.log.transactions {
        // Before applying a day's transactions, every earlier day must match.
        while let Some(snap) = snapshots.next_if(|s| s.date < tx.date) {
            assert_eq!(balances, snap.balances, "balances differ on {}", snap.date);
        }
        for (account, amt) in &tx.postings {
            if let Some(i) = output.accounts.iter().position(|a| a == &**account) {
                balances[i] += amt;
            }
        }
    }
    for snap in snapshots {
        assert_eq!(balances, snap.balances, "balances differ on {}", snap.date);
    }
}

// --- CLI ---

#[test]
fn cli_exits_cleanly_when_the_reader_stops_early() {
    use std::io::{BufRead, BufReader};
    use std::process::{Command, Stdio};

    let dir = std::env::temp_dir().join(format!("saldo-cli-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("model.saldo");
    std::fs::write(
        &path,
        "account A\naccount B\nentry daily \"x\" { A = 1\n B }\n",
    )
    .unwrap();

    let mut child = Command::new(env!("CARGO_BIN_EXE_saldo"))
        .arg(&path)
        .args(["--from", "2000-01-01", "--to", "2049-12-31"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut first_line = String::new();
    BufReader::new(child.stdout.take().unwrap())
        .read_line(&mut first_line)
        .unwrap();
    // Dropping stdout closes the pipe while saldo is still writing.
    let output = child.wait_with_output().unwrap();
    std::fs::remove_dir_all(&dir).ok();

    assert_eq!(first_line, "2000-01-01 opening-balances\n");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

// --- keywords ---

#[test]
fn keywords_are_lowercase_only() {
    // Capitalized keywords are ordinary names.
    let src = "
        account Assets:Cash
        account Income:Gifts
        param All = 5
        param If = 1
        schedule Monthly = monthly on the 1st
        entry Monthly \"Gift\" {
          Assets:Cash = All + If
          Income:Gifts
        }
    ";
    let output = run(src, &opts("2025-01-01", "2025-01-01")).unwrap();
    assert_eq!(posting(&output, 0, "Assets:Cash"), Decimal::new(6, 0));

    let errors = run(
        "param p = IF true THEN 1 ELSE 2",
        &opts("2025-01-01", "2025-01-01"),
    );
    assert!(errors.is_err());
}

// --- logical operators ---

fn assert_holds(condition: &str) {
    let src = format!("assert that {condition}");
    if let Err(errors) = run(&src, &opts("2025-01-01", "2025-01-01")) {
        panic!("`{condition}` failed: {errors:?}");
    }
}

#[test]
fn logical_operators() {
    assert_holds("1 < 2 and 2 < 3");
    assert_holds("1 > 2 or 2 < 3");
    assert_holds("not 1 > 2");
    // `and` binds tighter than `or`; `not` tighter than both.
    assert_holds("true or false and false");
    assert_holds("not false and true");
    assert_holds("(not (1 > 2)) == true");
    // Short-circuiting: the right side would divide by zero.
    assert_holds("not (false and 1 / 0 == 0)");
    assert_holds("true or 1 / 0 == 0");
}

#[test]
fn chained_comparisons_are_rejected() {
    let errors = run("assert that 1 < 2 < 3", &opts("2025-01-01", "2025-01-01")).unwrap_err();
    assert!(has_error(&errors, "comparisons can't be chained"));
}

#[test]
fn logical_operators_require_bools() {
    let errors = run("assert that 1 and true", &opts("2025-01-01", "2025-01-01")).unwrap_err();
    assert!(has_error(
        &errors,
        "an operand of `and` must be a bool, but this is a number"
    ));
}

// --- type checking ---

#[test]
fn type_errors_are_reported_before_simulating() {
    let cases = [
        // Only evaluated from 2040 on, but reported up front.
        (
            "account A\naccount B\nentry daily \"x\" {\n A = if Assets > 0 then true else 1\n B\n}\naccount Assets",
            "`if` branches must have the same type, but are a bool and a number",
        ),
        (
            "param p = 1 > 0",
            "a param must be a number, but this is a bool",
        ),
        (
            "assert that 1 + 1",
            "an assertion must be a bool, but this is a number",
        ),
        (
            "assert that 1 == true",
            "`==` compares a number with a bool",
        ),
        ("assert that not 1", "the operand of `not` must be a bool"),
        ("param p = -(1 > 0)", "the operand of `-` must be a number"),
        (
            "param p = if 1 then 2 else 3",
            "an `if` condition must be a bool",
        ),
        (
            "param p = max(1 > 0, 2)",
            "an argument to `max` must be a number",
        ),
        (
            "fn positive(x) { x > 0 }\nparam p = positive(1)",
            "a param must be a number, but this is a bool",
        ),
        (
            "fn f(x) { let y = x > 0; x }",
            "a `let` binding must be a number",
        ),
        (
            "account A = 1 > 0 @ 2025-01-01",
            "an opening balance must be a number",
        ),
    ];
    for (src, expected) in cases {
        let errors = run(src, &opts("2040-01-01", "2040-01-01")).unwrap_err();
        assert!(has_error(&errors, expected), "`{src}`: {errors:?}");
    }
}

#[test]
fn functions_can_return_bools() {
    let src = "
        fn positive(x) { x > 0 }
        param p = 5
        assert that positive(p) and not positive(-p)
    ";
    run(src, &opts("2025-01-01", "2025-01-01")).unwrap();
}

// --- missed aggregate warnings ---

const CAPPED_CONTRIBUTIONS: &str = "
    account Assets:Retirement
    account Income:Salary
    entry monthly on the 15th \"Paycheck\" {
      Assets:Retirement = min(1_000 - k401.ytd, 300) + k401.qtd * 0 as k401
      Income:Salary
    } as paycheck
    assert that paycheck.k401.ytd <= 1_000
";

fn warnings(src: &str, from: &str) -> Vec<String> {
    let output = run(src, &opts(from, "2025-12-31")).unwrap();
    output.warnings.iter().map(|w| w.message.clone()).collect()
}

#[test]
fn warns_when_an_aggregate_misses_earlier_firings() {
    let output = run(CAPPED_CONTRIBUTIONS, &opts("2025-05-20", "2025-12-31")).unwrap();
    let messages: Vec<&str> = output.warnings.iter().map(|w| w.message.as_str()).collect();
    // One warning per aggregate, even though `paycheck.k401.ytd` repeats it.
    assert_eq!(
        messages,
        [
            "`k401.ytd` is missing amounts from before 2025-05-20",
            "`k401.qtd` is missing amounts from before 2025-05-20",
        ]
    );
    let span = output.warnings[0].span;
    assert_eq!(&CAPPED_CONTRIBUTIONS[span.start..span.end], "k401.ytd");
    assert!(
        output.warnings[1].extra[0]
            .1
            .contains("would have fired on 2025-04-15")
    );
}

#[test]
fn no_warning_when_nothing_was_missed() {
    // Starting on the first day of the year.
    assert!(warnings(CAPPED_CONTRIBUTIONS, "2025-01-01").is_empty());
    // Starting mid-period, but before the entry first fires.
    assert!(warnings(CAPPED_CONTRIBUTIONS, "2025-01-10").is_empty());
    // Starting mid-year, but an account opening on Jan 1 warms up from there.
    let warmed_up = CAPPED_CONTRIBUTIONS.replace(
        "account Assets:Retirement",
        "account Assets:Retirement = 0 @ 2025-01-01",
    );
    assert!(warnings(&warmed_up, "2025-06-01").is_empty());
}

#[test]
fn unicode_account_names_work_and_align() {
    let src = "
        account Aktiva:Geld = 100 @ 2025-01-01
        account Ausgaben:Café
        entry daily \"Kaffee\" {
          Ausgaben:Café = 3
          Aktiva:Geld
        }
    ";
    let ledger = run(src, &opts("2025-01-01", "2025-01-01"))
        .unwrap()
        .to_ledger();
    assert!(
        ledger.contains("  Ausgaben:Café   3\n  Aktiva:Geld    -3\n"),
        "{ledger}"
    );
}

// --- rates ---

/// The sum of the postings to `account` dated in `year`.
fn posted_in(output: &saldo::Output, account: &str, year: i32) -> Decimal {
    use chrono::Datelike;
    output
        .log
        .transactions
        .iter()
        .filter(|tx| tx.date.year() == year)
        .flat_map(|tx| &tx.postings)
        .filter(|(p, _)| p.to_string() == account)
        .map(|(_, v)| *v)
        .sum()
}

/// The amounts posted to `account`, in order.
fn postings_to(output: &saldo::Output, account: &str) -> Vec<Decimal> {
    output
        .log
        .transactions
        .iter()
        .flat_map(|tx| &tx.postings)
        .filter(|(p, _)| p.to_string() == account)
        .map(|(_, v)| *v)
        .collect()
}

fn usd(amount: &str) -> Decimal {
    amount.parse().unwrap()
}

#[test]
fn per_year_amounts_add_up_exactly_whatever_the_number_of_paydays() {
    // 2021 starts and ends on a Friday, so it has 27 biweekly paydays; 2022
    // has 26.
    let src = "
        account Assets:Retirement:SemiMonthly
        account Assets:Retirement:Biweekly
        account Income:Salary
        param max_401k = 24_500 per year
        entry every month on the 15th and last day \"Semi-monthly\" {
          Assets:Retirement:SemiMonthly = max_401k
          Income:Salary
        }
        entry every second friday from 2021-01-01 \"Biweekly\" {
          Assets:Retirement:Biweekly = max_401k
          Income:Salary
        }
    ";
    let output = run(src, &opts("2021-01-01", "2022-12-31")).unwrap();
    for year in [2021, 2022] {
        for account in [
            "Assets:Retirement:SemiMonthly",
            "Assets:Retirement:Biweekly",
        ] {
            assert_eq!(
                posted_in(&output, account, year),
                usd("24500.00"),
                "{account} {year}"
            );
        }
    }
    let biweekly = postings_to(&output, "Assets:Retirement:Biweekly");
    assert_eq!(biweekly.len(), 27 + 26);
    assert_eq!(biweekly[0], usd("907.41")); // 24_500 / 27
    assert_eq!(biweekly[27], usd("942.31")); // 24_500 / 26
}

#[test]
fn a_raise_applies_from_the_next_paycheck() {
    let src = "
        account Assets:Cash
        account Income:Salary
        param salary {
          from 2026-01-01 to 2026-07-01 = 100_000 per year
          from 2026-07-01               = 110_000 per year
        }
        entry every month on the 15th and last day \"Paycheck\" {
          Assets:Cash = salary
          Income:Salary
        }
    ";
    let output = run(src, &opts("2026-01-01", "2026-12-31")).unwrap();
    let cash = postings_to(&output, "Assets:Cash");
    assert_eq!(cash[..2], [usd("4166.67"), usd("4166.66")]);
    assert_eq!(cash[12], usd("4583.33")); // 110_000 / 24
    assert_eq!(posted_in(&output, "Assets:Cash", 2026), usd("105000.00"));
}

#[test]
fn a_posting_spreads_rates_but_not_the_legs_it_subtracts() {
    let src = "
        account Assets:Cash
        account Assets:Retirement
        account Income:Salary
        param salary   = 120_000 per year
        param max_401k = 24_000 per year
        entry monthly \"Paycheck\" {
          Assets:Retirement = max_401k as contribution
          Assets:Cash       = salary - contribution
          Income:Salary
        }
    ";
    let output = run(src, &opts("2026-01-01", "2026-01-31")).unwrap();
    assert_eq!(posting(&output, 0, "Assets:Retirement"), usd("2000.00"));
    assert_eq!(posting(&output, 0, "Assets:Cash"), usd("8000.00"));
    assert_eq!(posting(&output, 0, "Income:Salary"), usd("-10000.00"));
}

#[test]
fn a_rate_minus_its_period_total_is_whats_left_of_it() {
    // Contribute 16% of each paycheck until the yearly limit is reached.
    let src = "
        account Assets:Retirement
        account Income:Salary
        param salary   = 300_000 per year
        param max_401k = 24_500 per year
        entry every month on the 15th and last day \"Paycheck\" {
          Assets:Retirement = min(salary * 0.16, max_401k - contribution.ytd) as contribution
          Income:Salary
        } as pay
        assert that pay.contribution.ytd <= max_401k
    ";
    let output = run(src, &opts("2026-01-01", "2026-12-31")).unwrap();
    let contributions = postings_to(&output, "Assets:Retirement");
    assert_eq!(
        contributions[..13],
        [[usd("2000.00"); 12].as_slice(), &[usd("500.00")]].concat()
    );
    assert_eq!(
        posted_in(&output, "Assets:Retirement", 2026),
        usd("24500.00")
    );
}

#[test]
fn interest_accrues_by_the_actual_days_in_the_year() {
    let src = "
        account Liabilities:Loan = -100_000 @ 2026-01-01
        account Liabilities:AccruedInterest
        account Expenses:Interest
        param rate = 5% per year
        entry daily \"Interest\" {
          Liabilities:AccruedInterest = Liabilities:Loan * rate
          Expenses:Interest
        }
    ";
    let output = run(src, &opts("2026-01-01", "2028-12-31")).unwrap();
    let interest = postings_to(&output, "Liabilities:AccruedInterest");
    assert_eq!(interest[0], usd("-13.70")); // 5_000 / 365
    // 2028 has 366 days.
    for year in [2026, 2027, 2028] {
        let total = posted_in(&output, "Liabilities:AccruedInterest", year);
        assert_eq!(total, usd("-5000.00"), "{year}");
    }
}

#[test]
fn a_schedule_starting_midyear_gets_part_of_a_yearly_amount() {
    let src = "
        account Assets:Cash
        account Income:Salary
        param salary = 120_000 per year
        entry every month from 2026-07-01 \"Paycheck\" {
          Assets:Cash = salary
          Income:Salary
        }
    ";
    let output = run(src, &opts("2026-01-01", "2026-12-31")).unwrap();
    assert_eq!(postings_to(&output, "Assets:Cash"), [usd("10000.00"); 6]);
}

#[test]
fn entries_firing_less_often_than_the_rate_collect_the_periods_between() {
    let src = "
        account Assets:Cash
        account Expenses:Rent
        account Expenses:Stipend
        param rent    = 1_000 per month
        param stipend = 200 per week
        entry quarterly \"Rent\" {
          Expenses:Rent = rent
          Assets:Cash
        }
        entry every second friday from 2026-01-09 \"Stipend\" {
          Expenses:Stipend = stipend
          Assets:Cash
        }
    ";
    let output = run(src, &opts("2026-01-01", "2026-12-31")).unwrap();
    assert_eq!(postings_to(&output, "Expenses:Rent"), [usd("3000.00"); 4]);
    assert_eq!(
        postings_to(&output, "Expenses:Stipend"),
        [usd("400.00"); 26]
    );
}

#[test]
fn per_converts_rates_between_months_and_years() {
    let src = "
        account Assets:Cash
        account Income:Salary
        param salary  = 120_000 per year
        param monthly = salary per month
        param raise = 1_200
        param raised  = (monthly + raise / 12) per year
        entry yearly \"Pay\" {
          Assets:Cash = monthly
          Income:Salary
        }
        entry monthly \"Raise\" {
          Assets:Cash = raised - salary
          Income:Salary
        }
    ";
    let output = run(src, &opts("2026-01-01", "2026-12-31")).unwrap();
    let pay = |label: &str| {
        let tx = output
            .log
            .transactions
            .iter()
            .position(|t| &*t.label == label);
        posting(&output, tx.unwrap(), "Assets:Cash")
    };
    assert_eq!(pay("Pay"), usd("120000.00"));
    assert_eq!(pay("Raise"), usd("100.00"));
}

#[test]
fn functions_pass_rates_through() {
    let src = "
        account Assets:Cash
        account Income:Salary
        fn net(gross, rate) { gross - gross * rate }
        param salary = 120_000 per year
        entry monthly \"Paycheck\" {
          Assets:Cash = net(salary, 0.25)
          Income:Salary
        }
    ";
    let output = run(src, &opts("2026-01-01", "2026-01-31")).unwrap();
    assert_eq!(posting(&output, 0, "Assets:Cash"), usd("7500.00"));
}

#[test]
fn rates_and_amounts_dont_mix_outside_postings() {
    let cases = [
        (
            "param x = salary + Assets:Cash",
            "`+` mixes an amount per year with an amount",
        ),
        (
            "assert that Assets:Cash >= salary",
            "`>=` mixes an amount with an amount per year",
        ),
        (
            "param x = salary + rent",
            "`+` mixes an amount per year with an amount per month",
        ),
        (
            "param x = salary * rent",
            "can't multiply an amount per year by an amount per month",
        ),
        (
            "param x = 1 / salary",
            "can't divide a number by an amount per year",
        ),
        (
            "account Assets:Odd = salary @ 2026-01-01",
            "an opening balance must be an amount, but this is an amount per year",
        ),
        (
            "param x = salary per week",
            "can't convert an amount per year to per week: \
             a year isn't a fixed number of weeks",
        ),
        (
            "param x = job.pay.ytd per year",
            "`per year` can't apply to a year-to-date total, \
             which is an amount so far, not per period",
        ),
        (
            "fn monthly(x) { x per month }\nparam x = monthly(salary)",
            "can't convert an amount per year to per month inside a function",
        ),
        (
            "param x { from 2026-01-01 to 2026-02-01 = salary\n from 2026-02-01 = Assets:Cash }",
            "param `x` is an amount per year in one interval but an amount in another",
        ),
        (
            "fn f(a, b) { a - b }\nparam x = f(salary, Assets:Cash)",
            "`-` mixes an amount per year with an amount",
        ),
    ];
    for (decl, message) in cases {
        let src = format!(
            "account Assets:Cash
             account Income:Salary
             param salary = 120_000 per year
             param rent   = 1_000 per month
             entry monthly \"Pay\" {{
               Assets:Cash = 1 as pay
               Income:Salary
             }} as job
             {decl}"
        );
        let errors = run(&src, &opts("2026-01-01", "2026-01-31")).unwrap_err();
        assert!(has_error(&errors, message), "{decl}: {errors:?}");
    }
}

#[test]
fn dividing_by_a_per_points_at_its_precedence() {
    let src = "param salary = 130_000 / 52 per week";
    let errors = run(src, &opts("2026-01-01", "2026-01-31")).unwrap_err();
    let [saldo::Error::Diagnostic(d)] = errors.as_slice() else {
        panic!("{errors:?}");
    };
    assert_eq!(d.message, "can't divide a number by an amount per week");
    assert!(
        d.extra[0].1.contains("`per` binds tighter than `/`"),
        "{d:?}"
    );
}

// --- fill ---

#[test]
fn fill_reaches_a_yearly_target_after_a_midyear_start() {
    let src = "
        account Assets:Cash
        account Assets:Retirement
        account Income:Salary
        account Income:OldSalary
        param salary   = 150_000 per year
        param max_401k = 24_500 per year
        param old_contribution {
          from 2026-01-01 to 2026-07-01 = 1_000 per month
          from 2026-07-01               = 0
        }
        entry monthly \"Old job\" {
          Assets:Retirement = old_contribution as contribution
          Income:OldSalary
        } as old_job
        entry every second friday from 2026-07-10 \"New job\" {
          Assets:Retirement = fill(max_401k - old_job.contribution.ytd) as contribution
          Assets:Cash       = salary - contribution
          Income:Salary
        }
    ";
    let output = run(src, &opts("2026-01-01", "2027-12-31")).unwrap();
    let new_job: Vec<Decimal> = output
        .log
        .transactions
        .iter()
        .filter(|tx| &*tx.label == "New job")
        .map(|tx| tx.postings[0].1)
        .collect();
    // 18_500 left after the old job, over the 13 paydays left in 2026.
    assert_eq!(new_job[0], usd("1423.08"));
    assert_eq!(new_job[..13].iter().sum::<Decimal>(), usd("18500.00"));
    // A full year: 26 paydays.
    assert_eq!(new_job[13], usd("942.31"));
    for year in [2026, 2027] {
        assert_eq!(
            posted_in(&output, "Assets:Retirement", year),
            usd("24500.00")
        );
    }
    // The salary is still spread, so half a year's worth arrives in 2026.
    assert_eq!(posted_in(&output, "Income:Salary", 2026), usd("-75000.00"));
}

#[test]
fn fill_makes_up_for_firings_held_back() {
    let src = "
        account Assets:Retirement
        account Income:Salary
        param cap {
          from 2026-01-01 to 2026-07-01 = 500
          from 2026-07-01               = 10_000
        }
        entry monthly \"Paycheck\" {
          Assets:Retirement = min(fill(24_000 per year), cap)
          Income:Salary
        }
    ";
    let output = run(src, &opts("2026-01-01", "2026-12-31")).unwrap();
    let contributions = postings_to(&output, "Assets:Retirement");
    assert_eq!(contributions[..6], [usd("500.00"); 6]);
    // (24_000 - 3_000) / 6 months left
    assert_eq!(contributions[6..], [usd("3500.00"); 6]);
}

#[test]
fn fill_has_to_be_what_a_posting_posts() {
    let cases = [
        (
            "param x = fill(max_401k)",
            "`fill` can only be the amount a posting posts",
        ),
        (
            "entry monthly \"A\" {\n Assets:Cash = fill(max_401k) + 100\n Income:Salary }",
            "`fill` can only be the amount a posting posts",
        ),
        (
            "entry monthly \"A\" {\n Assets:Cash = fill(500)\n Income:Salary }",
            "`fill` needs an amount per period",
        ),
        (
            "entry monthly \"A\" {\n Assets:Cash = fill(max_401k - paid.ytd) as paid\n Income:Salary }",
            "`fill` already subtracts what this posting has posted",
        ),
        (
            "fn f(x) { fill(x) }\nentry monthly \"A\" {\n Assets:Cash = f(max_401k)\n Income:Salary }",
            "`fill` can only be the amount a posting posts",
        ),
    ];
    for (decl, message) in cases {
        let src = format!(
            "account Assets:Cash
             account Income:Salary
             param max_401k = 24_500 per year
             {decl}"
        );
        let errors = run(&src, &opts("2026-01-01", "2026-01-31")).unwrap_err();
        assert!(has_error(&errors, message), "{decl}: {errors:?}");
    }
}

#[test]
fn fill_warns_when_the_simulation_starts_partway_through_its_period() {
    let src = "
        account Assets:Retirement
        account Income:Salary
        entry monthly \"Paycheck\" {
          Assets:Retirement = fill(24_000 per year)
          Income:Salary
        }
    ";
    let output = run(src, &opts("2026-03-15", "2026-12-31")).unwrap();
    assert_eq!(output.warnings.len(), 1);
    assert_eq!(
        output.warnings[0].message,
        "`fill` is missing what was posted before 2026-03-15"
    );
    let output = run(src, &opts("2026-01-01", "2026-12-31")).unwrap();
    assert!(output.warnings.is_empty());
}
