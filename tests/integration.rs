use chrono::NaiveDate;
use rust_decimal::Decimal;
use saldo::RunOpts;

fn d(s: &str) -> NaiveDate {
    NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap()
}

fn opts(from: &str, to: &str) -> RunOpts {
    RunOpts {
        from: d(from),
        to: d(to),
        seed: 0,
        every: saldo::TimeUnit::Day,
        series: Vec::new(),
    }
}

/// Runs `src`, treating a failed check as an error, as most tests want: their
/// checks say what must hold.
fn run(src: &str, opts: &RunOpts) -> Result<saldo::Output, Vec<saldo::Error>> {
    let output = saldo::run(src, opts)?;
    if output.log.failed_checks.is_empty() {
        Ok(output)
    } else {
        Err(output
            .log
            .failed_checks
            .iter()
            .map(|failure| saldo::Error::Diagnostic(failure.diagnostic()))
            .collect())
    }
}

fn path(name: &str) -> saldo::Path {
    saldo::Path(name.split('.').map(str::to_string).collect())
}

fn has_error(errors: &[saldo::Error], needle: &str) -> bool {
    errors
        .iter()
        .any(|e| matches!(e, saldo::Error::Diagnostic(d) if d.message.contains(needle)))
}

/// What transaction `tx` posts to `account`.
fn posting(output: &saldo::Output, tx: usize, account: &str) -> Decimal {
    output.log.transactions[tx]
        .postings
        .iter()
        .find(|(p, _)| p.to_string() == account)
        .map(|(_, v)| *v)
        .unwrap_or_else(|| panic!("no posting to {account} in transaction {tx}"))
}

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

// --- happy path ---

#[test]
fn ledger_output_contains_transactions() {
    let src = r#"
        balances @ 2025-01-01 { assets.cash = 1000 }
        entry paycheck "Paycheck" @ last {
          income.salary -> assets.cash : 500
        }
    "#;
    let output = run(src, &opts("2025-01-01", "2025-03-31")).unwrap();
    let ledger = output.to_ledger();
    assert!(ledger.contains("opening-balances"));
    assert!(
        ledger
            .lines()
            .any(|line| line.split_whitespace().eq(["Assets:Cash", "1000"]))
    );
    assert!(
        ledger.contains("2025-01-31 Paycheck\n  Assets:Cash     500\n  Income:Salary  -500\n"),
        "{ledger}"
    );
}

#[test]
fn csv_output_reads_each_kind_positive() {
    let src = "
        balances @ 2025-01-01 {
          liabilities.loan = 500
          assets.cash      = 200
        }
    ";
    let output = run(src, &opts("2025-01-01", "2025-01-03")).unwrap();
    let csv = output.to_csv();
    let lines: Vec<&str> = csv.lines().collect();
    assert_eq!(lines[0], r#""date","Assets:Cash","Liabilities:Loan""#);
    assert_eq!(lines[1], "2025-01-01,200.00,500.00");
    assert_eq!(lines.len(), 4); // header + 3 days
    // The ledger shows what's owed as negative, as ledger tools expect.
    assert!(output.to_ledger().contains("Liabilities:Loan        -500"));
}

#[test]
fn single_day_range_is_accepted() {
    let output = run(
        "balances @ 2025-06-01 { assets.cash = 100 }",
        &opts("2025-06-01", "2025-06-01"),
    )
    .unwrap();
    assert!(output.to_ledger().contains("opening-balances"));
}

#[test]
fn log_is_accessible_directly() {
    let src = r#"
        balances @ 2025-01-01 { assets.cash = 1000 }
        entry paycheck "Paycheck" @ last {
          income.salary -> assets.cash : 500
        }
    "#;
    let output = run(src, &opts("2025-01-01", "2025-01-31")).unwrap();
    assert_eq!(output.log.snapshots.len(), 31);
    assert_eq!(output.log.transactions.len(), 1);
    assert_eq!(&*output.log.transactions[0].label, "Paycheck");
}

#[test]
fn an_entry_without_a_label_is_named_by_its_path() {
    let src = "
        mod pay {
          entry salary @ last { income.salary -> assets.cash : 500 }
        }
    ";
    let output = run(src, &opts("2025-01-01", "2025-01-31")).unwrap();
    assert_eq!(&*output.log.transactions[0].label, "pay.salary");
}

// --- option validation ---

#[test]
fn from_after_to_returns_error() {
    let errors = run("", &opts("2025-12-31", "2025-01-01")).unwrap_err();
    assert!(matches!(errors[0], saldo::Error::InvalidDateRange { .. }));
}

// --- lex / parse errors ---

#[test]
fn lexer_error_is_a_diagnostic() {
    let errors = run("let x = 1\n$$$$", &opts("2025-01-01", "2025-01-31")).unwrap_err();
    assert!(matches!(errors[0], saldo::Error::Diagnostic(_)));
}

#[test]
fn parse_error_is_a_diagnostic() {
    let src = r#"
        entry broken "Broken" @ last {
          income.salary -> assets.cash : 100
    "#;
    let errors = run(src, &opts("2025-01-01", "2025-01-31")).unwrap_err();
    assert!(matches!(errors[0], saldo::Error::Diagnostic(_)));
}

// --- resolve errors ---

#[test]
fn reading_an_account_nothing_moves_is_a_diagnostic() {
    let src = "
        entry paycheck @ last { income.salary -> assets.cash : 500 }
        check assets.csh >= 0
    ";
    let errors = run(src, &opts("2025-01-01", "2025-01-31")).unwrap_err();
    assert!(has_error(
        &errors,
        "no entry or balance moves money into or out of `assets.csh`"
    ));
}

#[test]
fn unknown_name_in_expr_is_a_diagnostic() {
    let src = "entry paycheck @ last { income.salary -> assets.cash : ghost }";
    let errors = run(src, &opts("2025-01-01", "2025-01-31")).unwrap_err();
    assert!(has_error(&errors, "unknown name `ghost`"));
}

// --- runtime / check errors ---

#[test]
fn a_failing_check_is_reported_once_and_doesnt_stop_the_run() {
    let src = "
        balances @ 2025-01-01 { assets.cash = 100 }
        entry spend @ last { assets.cash -> expenses.x : 40 }
        check assets.cash >= 0
        check assets.cash >= 50 @ jun 30
        check assets.cash < 1_000
    ";
    let output = saldo::run(src, &opts("2025-01-01", "2025-12-31")).unwrap();
    assert_eq!(output.log.snapshots.len(), 365);
    let messages: Vec<String> = output
        .log
        .failed_checks
        .iter()
        .map(|failure| failure.diagnostic().message)
        .collect();
    assert_eq!(
        messages,
        [
            "check failed on 2025-03-31: -20 >= 0, and on 275 more days, last on 2025-12-31",
            "check failed on 2025-06-30: -140 >= 50",
        ]
    );
}

#[test]
fn checks_count_only_reported_days() {
    // Cash is negative in January, before the report starts.
    let src = "
        balances @ 2025-01-01 { assets.cash = -100 }
        entry pay @ feb 1 { income.x -> assets.cash : 200 }
        check assets.cash >= 0
    ";
    let output = saldo::run(src, &opts("2025-02-01", "2025-12-31")).unwrap();
    assert!(output.log.failed_checks.is_empty());
    let output = saldo::run(src, &opts("2025-01-30", "2025-12-31")).unwrap();
    assert_eq!(output.log.failed_checks[0].days, 2);
}

#[test]
fn an_error_in_a_check_still_stops_the_run() {
    let errors = saldo::run("check 1 / 0 > 0", &opts("2025-01-01", "2025-01-31")).unwrap_err();
    assert!(has_error(&errors, "division by zero"));
}

#[test]
fn passing_check_succeeds() {
    let src = "
        balances @ 2025-01-01 { assets.cash = 500 }
        check assets.cash >= 0
    ";
    run(src, &opts("2025-01-01", "2025-01-31")).unwrap();
}

#[test]
fn opening_balance_before_sim_start_warms_up() {
    let src = "
        balances @ 2024-01-01 { assets.cash = 1000 }
        entry paycheck @ last { income.salary -> assets.cash : 500 }
    ";
    // The warm-up applies 12 monthly entries, so the opening balance on
    // 2025-01-01 is 1000 + 12 * 500 = 7000.
    let output = run(src, &opts("2025-01-01", "2025-01-01")).unwrap();
    assert_eq!(
        output.log.opening[&path("assets.cash")],
        Decimal::new(7000, 0)
    );
}

#[test]
fn reference_before_opening_date_is_error() {
    let src = "
        balances @ 2025-06-01 { assets.cash = 1000 }
        entry paycheck @ last { income.salary -> assets.cash : 500 }
    ";
    // The entry fires in January, before `assets.cash` opens.
    let errors = run(src, &opts("2025-01-01", "2025-01-31")).unwrap_err();
    assert!(has_error(&errors, "opens on"));
}

#[test]
fn opening_balance_date_equals_sim_start() {
    let src = "
        balances @ 2025-03-01 { assets.cash = 500 }
        entry paycheck @ last { income.salary -> assets.cash : 100 }
    ";
    let output = run(src, &opts("2025-03-01", "2025-03-31")).unwrap();
    assert_eq!(
        output.log.opening[&path("assets.cash")],
        Decimal::new(500, 0)
    );
}

// --- moves and accounts ---

#[test]
fn moves_through_an_account_net_out_in_its_transaction() {
    let src = "
        let salary   = 120_000/year
        let max_401k = 24_000/year
        entry paycheck @ last {
          income.salary -> assets.cash       : salary
          assets.cash   -> assets.retirement : max_401k
        }
    ";
    let output = run(src, &opts("2026-01-01", "2026-01-31")).unwrap();
    assert_eq!(posting(&output, 0, "assets.retirement"), usd("2000.00"));
    assert_eq!(posting(&output, 0, "assets.cash"), usd("8000.00"));
    assert_eq!(posting(&output, 0, "income.salary"), usd("-10000.00"));
    assert_eq!(output.log.transactions[0].postings.len(), 3);
}

#[test]
fn accounts_read_positive_for_their_kind() {
    let src = "
        balances @ 2026-01-01 {
          assets.cash      = 1_000
          liabilities.loan = 3_000
        }
        entry salary @ 2026-01-15 { income.salary -> assets.cash : 500 }
        entry rent @ 2026-01-20 { assets.cash -> expenses.rent : 200 }
        check assets.cash == 1_300 and liabilities.loan == 3_000 @ 2026-01-31
        check income.salary == 500 and expenses.rent == 200 @ 2026-01-31
        check assets - liabilities == -1_700 @ 2026-01-31
    ";
    run(src, &opts("2026-01-01", "2026-01-31")).unwrap();
}

#[test]
fn a_group_reads_the_sum_of_its_accounts() {
    let src = "
        balances @ 2026-01-01 {
          assets.retirement.alex = 100
          assets.retirement.sam  = 200
        }
        balances @ 2026-06-01 { assets.retirement.new = 50 }
        // A group counts accounts that haven't opened yet as zero.
        check assets.retirement == 300 @ 2026-01-01
        check assets.retirement == 350 @ 2026-06-01
    ";
    run(src, &opts("2026-01-01", "2026-06-01")).unwrap();
}

#[test]
fn paying_off_an_account_with_its_balance() {
    let src = "
        balances @ 2025-01-01 { liabilities.accrued = 50 }
        entry clear @ * {
          assets.cash -> liabilities.accrued : liabilities.accrued
        }
    ";
    let output = run(src, &opts("2025-01-01", "2025-01-03")).unwrap();
    // Only the first day has a balance to clear; entries that move no money
    // are skipped.
    assert_eq!(output.log.transactions.len(), 1);
    assert_eq!(output.log.transactions[0].date, d("2025-01-01"));
    assert_eq!(posting(&output, 0, "liabilities.accrued"), usd("50"));
    assert_eq!(posting(&output, 0, "assets.cash"), usd("-50"));
}

#[test]
fn moving_nothing_posts_no_negative_zero() {
    let src = "
        entry clear {
          assets.cash -> liabilities.accrued : liabilities.accrued
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
fn opening_balances_are_rounded_like_moves() {
    let src = "
        balances @ 2025-01-01 { liabilities.accrued = 100 / 3 }
        entry clear {
          assets.cash -> liabilities.accrued : liabilities.accrued
        }
        check liabilities.accrued == 0
    ";
    let output = run(src, &opts("2025-01-01", "2025-01-01")).unwrap();
    assert_eq!(posting(&output, 0, "liabilities.accrued"), usd("33.33"));
}

#[test]
fn accounts_have_totals_like_tags() {
    let src = "
        entry paycheck @ 15th | last { income.salary -> assets.cash : 1_000 }
        entry rent @ 1st { assets.cash -> expenses.rent : 400 }
        check income.salary[year] == 6_000 and income.salary[month] == 2_000 @ 2026-03-31
        check assets.cash[quarter] == 4_800 @ 2026-03-31
        check income[year] == 6_000 @ 2026-03-31
    ";
    run(src, &opts("2026-01-01", "2026-03-31")).unwrap();
}

#[test]
fn an_account_cant_also_be_a_group() {
    let src = "
        entry a { income.x -> assets.cash : 1 }
        entry b { income.x -> assets.cash.checking : 1 }
    ";
    let errors = run(src, &opts("2025-01-01", "2025-01-01")).unwrap_err();
    assert!(has_error(
        &errors,
        "`assets.cash` holds money, so it can't also be a group"
    ));
}

#[test]
fn near_identical_accounts_warn() {
    let src = "
        entry a { income.salary -> assets.retirement : 1 }
        entry b { income.salary -> assets.retirment : 1 }
    ";
    let output = run(src, &opts("2025-01-01", "2025-01-01")).unwrap();
    assert_eq!(
        output.warnings[0].message,
        "`assets.retirement` and `assets.retirment` are both accounts; is one of them a typo?"
    );
}

#[test]
fn entries_fire_after_the_entries_that_move_what_they_read() {
    // `save` reads cash, which `paycheck` moves, so it fires after it, though
    // it's written first.
    let src = "
        balances @ 2025-01-01 { assets.cash = 100 }
        entry save @ 2025-01-02 {
          assets.cash -> assets.savings : assets.cash / 2
        }
        entry paycheck @ 2025-01-02 { income.salary -> assets.cash : 1_000 }
        entry rent @ 2025-01-02 { assets.cash -> expenses.rent : 10 }
    ";
    let output = run(src, &opts("2025-01-01", "2025-01-02")).unwrap();
    let labels: Vec<&str> = output.log.transactions.iter().map(|t| &*t.label).collect();
    // `rent` doesn't read anything, so it keeps its place after `paycheck`.
    assert_eq!(labels, ["paycheck", "rent", "save"]);
    assert_eq!(postings_to(&output, "assets.savings"), [usd("545")]);
}

#[test]
fn entries_that_read_each_others_moves_need_after() {
    let src = "
        balances @ 2025-01-01 { liabilities.loan = 10_000 }
        entry accrual {
          liabilities.accrued -> expenses.interest : liabilities.loan * 1%
        }
        entry payment @ 17th {
          assets.cash -> liabilities.accrued : liabilities.accrued
          assets.cash -> liabilities.loan    : 100
        }
    ";
    let errors = run(src, &opts("2025-01-01", "2025-01-31")).unwrap_err();
    assert!(has_error(
        &errors,
        "`accrual` and `payment` each read what the other moves, so it isn't clear which fires first"
    ));
    let rendered = saldo::format_errors("test.saldo", src, &errors, false);
    assert!(
        rendered.contains("add `after accrual` to `payment`"),
        "{rendered}"
    );

    // The payment clears what accrued before it, including that day's.
    let ordered = src.replace(
        "entry payment @ 17th {",
        "entry payment @ 17th after accrual {",
    );
    let output = run(&ordered, &opts("2025-01-01", "2025-01-31")).unwrap();
    assert_eq!(postings_to(&output, "liabilities.accrued")[17], usd("1700"));
    // The other way round, the accrual sees the paid-down loan.
    let reversed = src.replace("entry accrual {", "entry accrual after payment {");
    let output = run(&reversed, &opts("2025-01-01", "2025-01-31")).unwrap();
    let accrued = postings_to(&output, "liabilities.accrued");
    assert_eq!(accrued[16], usd("1600"));
    assert_eq!(accrued[17], usd("-99"));
}

#[test]
fn after_names_an_entry() {
    let cases = [
        (
            "entry a after b { income.x -> assets.y : 1 }",
            "unknown name `b`",
        ),
        (
            "let b = 1\nentry a after b { income.x -> assets.y : 1 }",
            "`b` isn't an entry",
        ),
        (
            "entry a after a { income.x -> assets.y : 1 }",
            "can't fire after itself",
        ),
    ];
    for (src, message) in cases {
        let errors = run(src, &opts("2025-01-01", "2025-01-01")).unwrap_err();
        assert!(has_error(&errors, message), "{src}: {errors:?}");
    }
}

#[test]
fn if_picks_which_moves_an_entry_makes() {
    let src = "
        balances @ 2026-01-01 { assets.cash = 15_000 }
        entry paycheck @ 15th { income.salary -> assets.cash : 4_000 }
        entry sweep @ last {
          if assets.cash > 20_000 {
            let extra = assets.cash - 20_000
            assets.cash -> assets.savings : extra
          } else if assets.cash < 5_000 {
            assets.savings -> assets.cash : 1_000
          } else {
            income.interest -> assets.cash : 1
          }
        }
    ";
    let output = run(src, &opts("2026-01-01", "2026-03-31")).unwrap();
    let labels: Vec<&str> = output.log.transactions.iter().map(|t| &*t.label).collect();
    assert_eq!(
        labels,
        [
            "paycheck", "sweep", "paycheck", "sweep", "paycheck", "sweep"
        ]
    );
    // 19,000 in January; 23,001 in February, of which 3,001 is swept.
    assert_eq!(postings_to(&output, "income.interest"), [usd("-1")]);
    assert_eq!(
        postings_to(&output, "assets.savings"),
        [usd("3001"), usd("4000")]
    );
}

#[test]
fn if_conditions_are_bools_and_lets_stay_in_their_branch() {
    let errors = run(
        "entry e { if 1 { income.a -> assets.b : 1 } }",
        &opts("2026-01-01", "2026-01-01"),
    )
    .unwrap_err();
    assert!(has_error(&errors, "an `if` condition must be a bool"));
    let errors = run(
        "entry e {\n if true { let x = 1 }\n income.a -> assets.b : x\n}",
        &opts("2026-01-01", "2026-01-01"),
    )
    .unwrap_err();
    assert!(has_error(&errors, "unknown name `x`"));
}

#[test]
fn for_moves_money_for_each_account_under_a_group() {
    let src = "
        balances @ 2026-01-01 {
          assets.retirement.alex = 10_000
          assets.retirement.sam  = 20_000
          assets.hsa             = 4_000
        }
        entry returns @ last {
          for a in assets.retirement, assets.hsa {
            let gain = a * 1%
            income.investments -> a : gain #returns
          }
        }
        check returns.returns[year] == 340 @ 2026-01-31
        check assets.retirement == 30_300 @ 2026-01-31
    ";
    let output = run(src, &opts("2026-01-01", "2026-01-31")).unwrap();
    assert_eq!(posting(&output, 0, "assets.retirement.alex"), usd("100"));
    assert_eq!(posting(&output, 0, "assets.retirement.sam"), usd("200"));
    assert_eq!(posting(&output, 0, "assets.hsa"), usd("40"));
    assert_eq!(posting(&output, 0, "income.investments"), usd("-340"));
}

#[test]
fn for_loops_nest_and_read_totals() {
    let src = "
        entry pay @ last {
          for who in income.salary {
            for to in assets.cash {
              who -> to : 100
            }
          }
        }
        entry b @ last {
          income.salary.bonus -> assets.cash.savings  : 1
          income.salary.base  -> assets.cash.checking : 1
        }
        entry report @ last after pay {
          for a in assets.cash {
            if a[year] > 150 { a -> expenses.fees : 1 }
          }
        }
    ";
    let output = run(src, &opts("2026-01-01", "2026-01-31")).unwrap();
    // Two incomes into two accounts: four moves of 100.
    assert_eq!(posting(&output, 0, "income.salary.bonus"), usd("-200"));
    assert_eq!(posting(&output, 0, "assets.cash.savings"), usd("200"));
    // Each account got more than 150 this year, so each pays a fee.
    assert_eq!(postings_to(&output, "expenses.fees"), [usd("2")]);
}

#[test]
fn for_needs_accounts_to_go_through() {
    let cases = [
        (
            "entry e { for a in assets.none { income.x -> a : 1 } }",
            "no entry or balance moves money into or out of `assets.none`",
        ),
        (
            "entry e { for a in salary { income.x -> a : 1 } }",
            "`salary` isn't an account",
        ),
        (
            "entry e { income.x -> assets.retirement : 1 }\nentry f { income.x -> assets.retirement.alex : 1 }",
            "use `for a in assets.retirement",
        ),
    ];
    for (src, message) in cases {
        let errors = run(src, &opts("2026-01-01", "2026-01-01")).unwrap_err();
        let rendered = saldo::format_errors("test.saldo", src, &errors, false);
        assert!(rendered.contains(message), "{src}: {rendered}");
    }
}

// --- functions ---

#[test]
fn fn_doubles_a_constant_value() {
    let src = "
        fn double(x) { x * 2 }
        let salary = 100
        let gross = double(salary)
        entry pay @ last { income.salary -> assets.cash : gross / 12 }
    ";
    let output = run(src, &opts("2025-01-01", "2025-01-31")).unwrap();
    assert_eq!(output.log.transactions.len(), 1);
    // gross = 200, gross/12 ≈ 16.67
    assert_eq!(posting(&output, 0, "assets.cash"), Decimal::new(1667, 2));
}

#[test]
fn fn_with_let_binding() {
    let src = "
        fn net(gross, rate) {
          let tax = gross * rate
          gross - tax
        }
        entry salary @ last { expenses.tax -> assets.cash : net(6000, 0.3) }
    ";
    let output = run(src, &opts("2025-01-01", "2025-01-31")).unwrap();
    // net(6000, 0.3) = 6000 - 1800 = 4200
    assert_eq!(posting(&output, 0, "assets.cash"), Decimal::new(4200, 0));
}

#[test]
fn fn_with_time_varying_value() {
    let src = "
        fn double(x) { x * 2 }
        let salary {
          2025-01-01..2025-07-01 => 100
          2025-07-01..           => 200
        }
        let doubled = double(salary)
        entry pay @ last { income.salary -> assets.cash : doubled }
    ";
    let output = run(src, &opts("2025-01-01", "2025-12-31")).unwrap();
    assert_eq!(output.log.transactions.len(), 12);
    assert_eq!(posting(&output, 0, "assets.cash"), Decimal::new(200, 0));
    assert_eq!(posting(&output, 6, "assets.cash"), Decimal::new(400, 0));
}

#[test]
fn fn_calling_another_fn_and_builtins() {
    let src = "
        fn double(x) { x * 2 }
        fn quad(x)   { double(double(x)) }
        fn positive(x) { max(x, 0) }
        entry pay @ last { income.salary -> assets.cash : quad(10) + positive(-50) + positive(30) }
    ";
    let output = run(src, &opts("2025-01-01", "2025-01-31")).unwrap();
    assert_eq!(posting(&output, 0, "assets.cash"), Decimal::new(70, 0));
}

#[test]
fn fn_with_if_expr() {
    let src = "
        fn bonus(salary, target) {
          if target > 0 then salary * 0.1 else 0
        }
        entry bonus_pay @ last { income.bonus -> assets.cash : bonus(10000, 1) }
    ";
    let output = run(src, &opts("2025-01-01", "2025-01-31")).unwrap();
    assert_eq!(posting(&output, 0, "assets.cash"), Decimal::new(1000, 0));
}

#[test]
fn functions_read_values_but_not_balances() {
    let src = "
        let rate = 0.3
        fn taxed(x) { x * rate }
        entry pay @ last { income.salary -> assets.cash : taxed(1_000) }
    ";
    let output = run(src, &opts("2025-01-01", "2025-01-31")).unwrap();
    assert_eq!(posting(&output, 0, "assets.cash"), usd("300"));

    let src = "
        balances @ 2025-01-01 { assets.cash = 1 }
        fn bad(x) { x * assets.cash }
    ";
    let errors = run(src, &opts("2025-01-01", "2025-01-01")).unwrap_err();
    assert!(has_error(&errors, "function `bad` can't read balances"));
}

#[test]
fn recursive_fn_is_rejected() {
    let errors = run(
        "fn loop_(x) { loop_(x) }",
        &opts("2025-01-01", "2025-01-01"),
    )
    .unwrap_err();
    assert!(has_error(&errors, "function `loop_` calls itself"));
}

#[test]
fn duplicate_fn_name_is_rejected() {
    let src = "fn f(x) { x }\nfn f(y) { y }";
    let errors = run(src, &opts("2025-01-01", "2025-01-01")).unwrap_err();
    assert!(has_error(&errors, "`f` is already declared"));
}

#[test]
fn fn_arity_mismatch_is_rejected() {
    let src = "
        fn double(x) { x * 2 }
        entry pay @ last { income.salary -> assets.cash : double(1, 2) }
    ";
    let errors = run(src, &opts("2025-01-01", "2025-01-01")).unwrap_err();
    assert!(has_error(&errors, "`double` takes 1 argument, got 2"));
}

#[test]
fn functions_take_arguments_by_name() {
    let src = "
        fn net(gross, rate) { gross - gross * rate }
        entry pay @ last { income.salary -> assets.cash : net(rate: 0.25, gross: 1_000) }
    ";
    let output = run(src, &opts("2026-01-01", "2026-01-31")).unwrap();
    assert_eq!(posting(&output, 0, "assets.cash"), usd("750"));
    let cases = [
        ("net(1, gross: 2)", "`gross` is given twice"),
        (
            "net(gross: 1, 2)",
            "arguments given by position go before those given by name",
        ),
        ("net(gross: 1, tax: 2)", "`net` has no parameter `tax`"),
        ("net(gross: 1)", "`net` needs `rate`"),
        ("min(a: 1, b: 2)", "`min` takes its arguments by position"),
    ];
    for (call, message) in cases {
        let src = format!("fn net(gross, rate) {{ gross - gross * rate }}\nlet x = {call}");
        let errors = run(&src, &opts("2026-01-01", "2026-01-01")).unwrap_err();
        assert!(has_error(&errors, message), "{call}: {errors:?}");
    }
}

const LOAN: &str = "
    fn loan(debt, interest, rate, payment) {
      mod {
        entry repay @ last {
          let accrued = debt * rate
          debt        -> interest : accrued
          assets.cash -> debt     : min(payment, debt + accrued) #paid
        }
      }
    }
";

#[test]
fn a_function_can_make_a_module() {
    let src = format!(
        "{LOAN}
        balances @ 2026-01-01 {{
          liabilities.loans.car       = 1_000
          liabilities.loans.furniture = 2_000
        }}
        mod car = loan(liabilities.loans.car, expenses.interest.car, 12%/year, 300)
        mod furniture = loan(
          debt:     liabilities.loans.furniture,
          interest: expenses.interest.furniture,
          rate:     6%/year,
          payment:  500,
        )
        check car.repay.paid[year] <= 1_100
    "
    );
    let output = run(&src, &opts("2026-01-01", "2026-12-31")).unwrap();
    let labels: Vec<&str> = output.log.transactions[..2]
        .iter()
        .map(|t| &*t.label)
        .collect();
    assert_eq!(labels, ["car.repay", "furniture.repay"]);
    assert_eq!(postings_to(&output, "expenses.interest.car")[0], usd("10"));
    assert_eq!(
        postings_to(&output, "expenses.interest.furniture")[0],
        usd("10")
    );
    // The car loan is paid down by 300 a month, less interest, and paid off
    // in April.
    let car: Vec<Decimal> = postings_to(&output, "liabilities.loans.car");
    assert_eq!(
        car[..4],
        [usd("290"), usd("292.90"), usd("295.83"), usd("121.27")]
    );
    assert_eq!(car[..4].iter().sum::<Decimal>(), usd("1000"));
}

#[test]
fn arguments_mean_what_they_mean_where_the_call_is() {
    let src = format!(
        "{LOAN}
        let rate = 100%/year
        mod loans {{
          let rate = 12%/year
          mod car = loan(liabilities.car, expenses.interest, rate, 0)
        }}
        balances @ 2026-01-01 {{ liabilities.car = 1_000 }}
    "
    );
    let output = run(&src, &opts("2026-01-01", "2026-01-31")).unwrap();
    assert_eq!(posting(&output, 0, "expenses.interest"), usd("10"));
    assert_eq!(&*output.log.transactions[0].label, "loans.car.repay");
}

#[test]
fn a_function_can_take_a_set_of_days() {
    let src = "
        fn bill(account, amount, days) {
          mod {
            entry pay @ days { assets.cash -> account : amount }
          }
        }
        mod rent = bill(expenses.rent, 2_000, 1st)
        mod gym = bill(expenses.gym, 50, 15th | last)
    ";
    let output = run(src, &opts("2026-01-01", "2026-01-31")).unwrap();
    let paid: Vec<(NaiveDate, &str)> = output
        .log
        .transactions
        .iter()
        .map(|t| (t.date, &*t.label))
        .collect();
    assert_eq!(
        paid,
        [
            (d("2026-01-01"), "rent.pay"),
            (d("2026-01-15"), "gym.pay"),
            (d("2026-01-31"), "gym.pay")
        ]
    );
}

#[test]
fn module_function_errors() {
    let cases = [
        (
            "mod car = loan(liabilities.car, expenses.x, 1%)",
            "`loan` needs `payment`",
        ),
        (
            "let x = loan(liabilities.car, expenses.x, 1%, 1)",
            "`loan` makes a module",
        ),
        (
            "fn f(x) { x }\nmod m = f(1)",
            "`f` returns a value, not a module",
        ),
        ("mod m = nope(1)", "unknown function `nope`"),
        (
            "mod car = loan(5, expenses.x, 1%, 1)",
            "`debt` is used as an account, so it needs one",
        ),
        (
            "fn g(x) { mod { mod again = g(x) } }\nmod m = g(1)",
            "`g` makes a module that uses `g` itself",
        ),
        (
            "fn h(rate) { mod { let rate = 1 } }\nmod m = h(1)",
            "`rate` is a parameter or `let` of `h`, so its module can't declare `rate` too",
        ),
    ];
    for (decl, message) in cases {
        let src = format!("{LOAN}\n{decl}");
        let errors = run(&src, &opts("2026-01-01", "2026-01-01")).unwrap_err();
        assert!(has_error(&errors, message), "{decl}: {errors:?}");
    }
}

// --- values & opening balances ---

#[test]
fn value_is_an_error_once_its_interval_ends() {
    let src = "
        let bonus { 2025-01-01..2025-01-02 => 1000 }
        entry bonus_pay @ * { income.bonus -> assets.cash : bonus }
    ";
    let errors = run(src, &opts("2025-01-01", "2025-01-03")).unwrap_err();
    assert!(has_error(&errors, "`bonus` has no value on 2025-01-02"));
}

#[test]
fn unused_value_without_a_value_is_not_an_error() {
    let src = "
        let bonus { 2025-01-01..2025-01-02 => 1000 }
        let half = bonus / 2
    ";
    run(src, &opts("2025-01-01", "2025-01-03")).unwrap();
}

#[test]
fn opening_balance_can_reference_a_value() {
    let src = "
        let initial = 5_000
        balances @ 2025-01-01 { assets.cash = initial }
    ";
    let output = run(src, &opts("2025-01-01", "2025-01-01")).unwrap();
    assert_eq!(
        output.log.opening.get(&path("assets.cash")),
        Some(&Decimal::new(5000, 0))
    );
}

#[test]
fn values_cant_read_balances_or_totals() {
    let cases = [
        (
            "let doubled = assets.savings * 2",
            "values can't read balances",
        ),
        ("let paid = pay.k401[year]", "values can't read totals"),
        (
            "let income_so_far = income.salary[year]",
            "values can't read totals",
        ),
        (
            "let grown { 2026.. => 1 compounding assets.savings every year }",
            "values can't read balances",
        ),
    ];
    for (decl, message) in cases {
        let src = format!(
            "balances @ 2025-01-01 {{ assets.savings = 10 }}
             entry pay @ last {{ income.salary -> assets.cash : 1 #k401 }}
             {decl}"
        );
        let errors = run(&src, &opts("2025-01-01", "2025-01-01")).unwrap_err();
        assert!(has_error(&errors, message), "{decl}: {errors:?}");
    }
}

#[test]
fn entry_lets_read_balances_once_they_open() {
    let src = r#"
        balances @ 2025-01-03 { assets.savings = 10 }
        entry interest "Interest" @ 2025-01-03 {
          let doubled = assets.savings * 2
          income.interest -> assets.cash : doubled
        }
    "#;
    let output = run(src, &opts("2025-01-01", "2025-01-03")).unwrap();
    let tx = output
        .log
        .transactions
        .iter()
        .position(|t| &*t.label == "Interest")
        .unwrap();
    assert_eq!(posting(&output, tx, "assets.cash"), Decimal::new(20, 0));
}

#[test]
fn entries_see_earlier_entries_that_day() {
    let src = "
        balances @ 2025-01-01 { assets.cash = 100 }
        entry paycheck @ 2025-01-02 { income.salary -> assets.cash : 1_000 }
        entry save @ 2025-01-02 {
          let half = assets.cash / 2
          assets.cash -> assets.savings : half
        }
        check assets.savings == 550 @ 2025-01-02
    ";
    let output = run(src, &opts("2025-01-01", "2025-01-02")).unwrap();
    assert_eq!(postings_to(&output, "assets.savings"), [usd("550")]);
}

#[test]
fn totals_include_earlier_entries_that_day() {
    let src = "
        entry paycheck @ last {
          income.salary -> assets.cash       : 1_000
          assets.cash   -> assets.retirement : 100 #k401
        }
        entry year_end @ last {
          let deducted = paycheck.k401[year]
          assets.cash -> expenses.tax  : deducted
          assets.cash -> expenses.fees : paycheck.k401[year]
        }
    ";
    let output = run(src, &opts("2025-01-01", "2025-02-28")).unwrap();
    assert_eq!(
        postings_to(&output, "expenses.tax"),
        [usd("100"), usd("200")]
    );
    assert_eq!(
        postings_to(&output, "expenses.fees"),
        [usd("100"), usd("200")]
    );
}

#[test]
fn account_opening_after_start_gets_an_opening_transaction() {
    let src = "balances @ 2025-01-02 { assets.cash = 1000 }";
    let output = run(src, &opts("2025-01-01", "2025-01-03")).unwrap();
    let tx = &output.log.transactions[0];
    assert_eq!(tx.date, d("2025-01-02"));
    assert_eq!(posting(&output, 0, "assets.cash"), Decimal::new(1000, 0));
    assert_eq!(
        posting(&output, 0, "equity.opening_balances"),
        Decimal::new(-1000, 0)
    );
}

#[test]
fn an_account_opens_once() {
    let src = "
        balances @ 2025-01-01 { assets.cash = 1 }
        balances @ 2025-02-01 { assets.cash = 2 }
    ";
    let errors = run(src, &opts("2025-01-01", "2025-01-01")).unwrap_err();
    assert!(has_error(
        &errors,
        "`assets.cash` already has an opening balance"
    ));
}

// --- hostile input ---

#[test]
fn arithmetic_overflow_is_a_diagnostic() {
    let src = "
        balances @ 2025-01-01 { assets.cash = 1 }
        entry grow { income.magic -> assets.cash : assets.cash * 1_000_000 }
    ";
    let errors = run(src, &opts("2025-01-01", "2025-12-31")).unwrap_err();
    assert!(has_error(&errors, "overflow"));
}

#[test]
fn non_ascii_outside_a_string_is_a_diagnostic() {
    for src in ["check Caf€", "let p = 1s€"] {
        let errors = run(src, &opts("2025-01-01", "2025-01-01")).unwrap_err();
        assert!(has_error(&errors, "unexpected character `€`"), "{src}");
        // Rendering must not split the multi-byte character.
        saldo::format_errors("test.saldo", src, &errors, true);
    }
}

#[test]
fn deeply_nested_expressions_are_rejected() {
    let parens = format!("let p = {}1{}", "(".repeat(20_000), ")".repeat(20_000));
    let negations = format!("let p = {}1", "-".repeat(20_000));
    let chain = format!("let p = {}", vec!["1"; 20_000].join(" + "));
    for src in [parens, negations, chain] {
        let errors = run(&src, &opts("2025-01-01", "2025-01-01")).unwrap_err();
        assert!(has_error(&errors, "nested too deeply"));
    }
}

#[test]
fn expressions_near_the_nesting_limit_evaluate() {
    let src = format!(
        "let p = {}1{}\nlet q = {}\ncheck p + q == 251",
        "(".repeat(250),
        ")".repeat(250),
        vec!["1"; 250].join(" + "),
    );
    run(&src, &opts("2025-01-01", "2025-01-01")).unwrap();
}

// --- name resolution ---

#[test]
fn invalid_date_is_a_diagnostic() {
    let errors = run("let p = 2025-13-01", &opts("2025-01-01", "2025-01-01")).unwrap_err();
    assert!(has_error(&errors, "invalid date `2025-13-01`"));
}

#[test]
fn fn_named_like_a_builtin_is_rejected() {
    let errors = run("fn min(a, b) { a + b }", &opts("2025-01-01", "2025-01-01")).unwrap_err();
    assert!(has_error(&errors, "`min` is a built-in function"));
}

#[test]
fn value_cycles_are_reported_once_with_their_path() {
    let src = "
        let a = b
        let b = c + 1
        let c = a
        let d = a
        let e = e * 2
    ";
    let errors = run(src, &opts("2025-01-01", "2025-01-01")).unwrap_err();
    assert_eq!(errors.len(), 2, "{errors:?}");
    assert!(has_error(&errors, "`a` depends on itself (a → b → c → a)"));
    assert!(has_error(&errors, "`e` depends on itself (e → e)"));
}

#[test]
fn modules_name_their_contents_in_full() {
    let src = r#"
        let rate = 1
        mod loans {
          let rate = 2
          mod car {
            let payment = 300
            entry repay "Car loan" @ last {
              assets.cash -> liabilities.car : payment * loans.rate * rate
            }
          }
        }
        check loans.car.payment == 300
    "#;
    let output = run(src, &opts("2025-01-01", "2025-01-31")).unwrap();
    // `rate` in `loans.car` is the one at the top: modules don't see their
    // parents' names.
    assert_eq!(posting(&output, 0, "liabilities.car"), usd("600"));
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
            "entry x { income.a -> assets.b : 1 }\ncheck assets.nowhere >= 0",
            "no entry or balance moves money",
            "assets.nowhere",
        ),
        (
            "entry x { cash -> assets.b : 1 }",
            "isn't an account",
            "cash",
        ),
        (
            "let p {\n 2025-01-01.. => 1\n 2025-06-01.. => 2\n}",
            "never applies",
            "2025-06-01.. => 2",
        ),
        (
            "let p {\n 2025-06-01..2025-01-01 => 1\n}",
            "ends before it starts",
            "2025-06-01..2025-01-01",
        ),
        (
            "entry x @ payday { income.a -> assets.b : 1 }",
            "unknown name `payday`",
            "payday",
        ),
        ("fn f(x, y, x) { x }", "already a parameter", "x"),
        (
            "entry pay { income.a -> assets.b : 1 #t }\nfn f(x) { pay.t[year] }",
            "can't read totals",
            "pay.t[year]",
        ),
        (
            "let payday = 15th | last\nlet x = payday * 2",
            "is a set of days",
            "payday",
        ),
    ];
    for (src, needle, expected) in cases {
        let errors = run(src, &opts("2025-01-01", "2025-01-01")).unwrap_err();
        assert_eq!(highlighted(src, &errors, needle), expected, "{src}");
    }
}

#[test]
fn arm_errors_point_at_the_offending_text() {
    let cases = [
        (
            "let p {\n jul 2027..=2026 => 1\n}",
            "ends before it starts",
            "jul 2027..=2026",
        ),
        (
            "let p {\n may..=2027 => 1\n}",
            "both ends of a range need a year",
            "may..=2027",
        ),
        (
            "let p {\n may.. => 1\n}",
            "a range of months needs an end",
            "may..",
        ),
        ("let p {\n 26 => 1\n}", "year written in full", "26"),
        (
            "let p {\n during => 1\n}",
            "unknown name `during`",
            "during",
        ),
        (
            "let p {\n _ => 1\n may => 2\n}",
            "comes after `_`",
            "may => 2",
        ),
        (
            "let p {\n q2 => 1\n may => 2\n}",
            "never applies",
            "may => 2",
        ),
        (
            "let p {\n 2027 => 1\n 2027-03-01..2027-04-01 => 2\n}",
            "never applies",
            "2027-03-01..2027-04-01 => 2",
        ),
        (
            "let p {\n jan..=jun | jul..=dec => 1\n _ => 2\n}",
            "never applies",
            "_ => 2",
        ),
    ];
    for (src, needle, expected) in cases {
        let errors = run(src, &opts("2025-01-01", "2025-01-01")).unwrap_err();
        assert_eq!(highlighted(src, &errors, needle), expected, "{src}");
    }
}

#[test]
fn arms_repeat_every_year_and_the_first_match_wins() {
    let src = "
        let tuition {
          jan 2027        => 0
          jul | aug       => 0
          2027-09-01..    => 2_100
          _               => 1_950
        }
        entry tuition_pay @ 1st { assets.cash -> expenses.tuition : tuition }
    ";
    let output = run(src, &opts("2026-10-01", "2027-10-31")).unwrap();
    let paid: Vec<(NaiveDate, Decimal)> = output
        .log
        .transactions
        .iter()
        .map(|tx| (tx.date, tx.postings[0].1))
        .collect();
    let mut expected: Vec<(NaiveDate, Decimal)> = [
        "2026-10-01",
        "2026-11-01",
        "2026-12-01",
        "2027-02-01",
        "2027-03-01",
        "2027-04-01",
        "2027-05-01",
        "2027-06-01",
    ]
    .into_iter()
    .map(|date| (d(date), usd("1950")))
    .collect();
    expected.extend(
        ["2027-09-01", "2027-10-01"]
            .into_iter()
            .map(|date| (d(date), usd("2100"))),
    );
    assert_eq!(paid, expected);
}

#[test]
fn arms_take_quarters_years_and_month_ranges() {
    let src = "
        let power {
          q3 2027   => 300/month
          dec..=feb => 900/month
          2026      => 100/month
          _         => 400/month
        }
        entry power_bill @ last { assets.cash -> expenses.power : power }
    ";
    let output = run(src, &opts("2026-11-01", "2027-10-31")).unwrap();
    let expected = [100, 900, 900, 900, 400, 400, 400, 400, 300, 300, 300, 400].map(Decimal::from);
    assert_eq!(postings_to(&output, "expenses.power"), expected);
}

const QUARTER_ENDS: &str = "mar 31 | jun 30 | sep 30 | dec 31";

#[test]
fn each_period_rolled_into_a_firing_gets_its_own_value() {
    let src = format!(
        r#"
        let power {{
          jan => 100/month
          feb => 200/month
          _   => 300/month
        }}
        entry bill "Power bill" @ {QUARTER_ENDS} {{
          assets.cash -> expenses.power : power
        }}
        entry bill_through_a_let "Power bill too" @ {QUARTER_ENDS} {{
          let amount = power * 2
          assets.cash -> expenses.power_too : amount
        }}
    "#
    );
    let output = run(&src, &opts("2027-01-01", "2027-06-30")).unwrap();
    assert_eq!(
        postings_to(&output, "expenses.power"),
        [usd("600"), usd("900")]
    );
    assert_eq!(
        postings_to(&output, "expenses.power_too"),
        [usd("1200"), usd("1800")]
    );
}

#[test]
fn a_rate_of_a_balance_changing_in_periods_without_a_firing_is_an_error() {
    // The balance only has today's value, so the months before can't each
    // get their own.
    let src = format!(
        r#"
        balances @ 2027-01-01 {{ liabilities.loan = 1_000 }}
        let interest {{
          jan => 1%/month
          feb => 2%/month
          _   => 3%/month
        }}
        entry bill "Interest" @ {QUARTER_ENDS} {{
          liabilities.loan -> expenses.interest : liabilities.loan * interest
        }}
    "#
    );
    let errors = run(&src, &opts("2027-01-01", "2027-03-31")).unwrap_err();
    assert_eq!(
        highlighted(
            &src,
            &errors,
            "`interest` changes during the months before 2027-03-01"
        ),
        "liabilities.loan * interest"
    );
}

#[test]
fn a_value_may_change_where_every_period_has_a_firing() {
    let src = format!(
        "
        let rent {{
          2027-03-10.. => 2_000/month
          _            => 1_800/month
        }}
        let insurance {{
          q1 => 300/quarter
          _  => 600/quarter
        }}
        entry rent_pay @ 15th {{ assets.cash -> expenses.rent : rent }}
        entry insurance_pay @ {QUARTER_ENDS} {{ assets.cash -> expenses.insurance : insurance }}
    "
    );
    let output = run(&src, &opts("2027-01-01", "2027-06-30")).unwrap();
    let rent = [1_800, 1_800, 2_000, 2_000, 2_000, 2_000].map(Decimal::from);
    assert_eq!(postings_to(&output, "expenses.rent"), rent);
    let insurance = [300, 600].map(Decimal::from);
    assert_eq!(postings_to(&output, "expenses.insurance"), insurance);
}

#[test]
fn a_schedule_in_some_months_splits_a_yearly_rate_over_its_firings() {
    let src = "
        entry tuition @ 1st & sep..=jun {
          assets.cash -> expenses.tuition : 24_000/year
        }
    ";
    let output = run(src, &opts("2027-01-01", "2027-12-31")).unwrap();
    let months: Vec<u32> = output
        .log
        .transactions
        .iter()
        .map(|tx| {
            use chrono::Datelike;
            tx.date.month()
        })
        .collect();
    assert_eq!(months, [1, 2, 3, 4, 5, 6, 9, 10, 11, 12]);
    assert_eq!(postings_to(&output, "expenses.tuition"), [usd("2400"); 10]);
}

#[test]
fn periods_a_schedule_leaves_out_dont_roll_into_the_next_firing() {
    let src = "
        entry monthly @ 1st & sep..=jun {
          assets.cash -> expenses.monthly : 1_000/month
        }
        // July 2 is a Friday, so the week of June 28 has days in June but
        // its firing is left out.
        entry weekly @ fri & sep..=jun {
          assets.cash -> expenses.weekly : 100/week
        }
    ";
    let output = run(src, &opts("2027-06-01", "2027-09-30")).unwrap();
    let on = |date: &str, account: &str| {
        let tx = output
            .log
            .transactions
            .iter()
            .position(|tx| tx.date == d(date) && tx.postings[0].0.to_string() == account)
            .unwrap_or_else(|| panic!("nothing posted to {account} on {date}"));
        posting(&output, tx, account)
    };
    assert_eq!(on("2027-09-01", "expenses.monthly"), usd("1000"));
    assert_eq!(on("2027-06-25", "expenses.weekly"), usd("100"));
    assert_eq!(on("2027-09-03", "expenses.weekly"), usd("100"));
}

#[test]
fn a_counted_schedule_keeps_its_rhythm_through_months_it_leaves_out() {
    let src = "
        entry pay @ 2027-01-01.. & fri / 2 & sep..=jun {
          income.pay -> assets.cash : 100
        }
    ";
    let output = run(src, &opts("2027-06-01", "2027-09-30")).unwrap();
    let dates: Vec<NaiveDate> = output.log.transactions.iter().map(|tx| tx.date).collect();
    assert_eq!(
        dates,
        ["2027-06-04", "2027-06-18", "2027-09-10", "2027-09-24"].map(d)
    );
}

#[test]
fn day_sets_combine_with_operators() {
    let src = "
        let payday = 15th | last
        entry pay @ payday - dec 31 & 2026-11-01.. {
          income.pay -> assets.cash : 1
        }
        entry gym @ 2026-12-01.. & * / 10 {
          assets.cash -> expenses.gym : 1
        }
    ";
    let output = run(src, &opts("2026-11-01", "2026-12-31")).unwrap();
    let dates = |label: &str| -> Vec<NaiveDate> {
        output
            .log
            .transactions
            .iter()
            .filter(|tx| &*tx.label == label)
            .map(|tx| tx.date)
            .collect()
    };
    assert_eq!(
        dates("pay"),
        ["2026-11-15", "2026-11-30", "2026-12-15"].map(d)
    );
    assert_eq!(
        dates("gym"),
        ["2026-12-01", "2026-12-11", "2026-12-21", "2026-12-31"].map(d)
    );
}

#[test]
fn schedules_can_end_and_arms_can_use_any_days() {
    let src = "
        let tip {
          fri  => 5
          last => 20
          _    => 0
        }
        entry tips @ 2026-01-01..2026-02-01 & (fri | last) {
          assets.cash -> expenses.tips : tip
        }
    ";
    let output = run(src, &opts("2026-01-01", "2026-03-31")).unwrap();
    // Fridays in January 2026, then the 31st; nothing after the range ends.
    assert_eq!(
        postings_to(&output, "expenses.tips"),
        ["5", "5", "5", "5", "5", "20"].map(usd)
    );
    assert_eq!(
        output.log.transactions.last().unwrap().date,
        d("2026-01-31")
    );
}

#[test]
fn every_nth_without_a_start_is_reported_once() {
    let src = "
        let biweekly = sun / 2
        entry x @ biweekly { income.a -> assets.b : 1 }
        check assets.b >= 0 @ biweekly
    ";
    let errors = run(src, &opts("2025-01-01", "2025-01-01")).unwrap_err();
    assert_eq!(errors.len(), 1, "{errors:?}");
    assert!(has_error(&errors, "no date to count from"));
}

#[test]
fn compounding_steps_up_on_each_anniversary_of_the_arms_start() {
    let src = "
        let tuition {
          jul 2026.. => 1_000 compounding 10% every year
        }
        let inflation = 3%
        let prices {
          2026.. => 1 compounding inflation every year
        }
        entry tuition_pay @ jun 30 | jul 1 { assets.cash -> expenses.tuition : tuition }
        entry other @ dec 31 { assets.cash -> expenses.other : 10_000 * prices }
    ";
    let output = run(src, &opts("2026-07-01", "2028-12-31")).unwrap();
    assert_eq!(
        postings_to(&output, "expenses.tuition"),
        ["1000", "1000", "1100", "1100", "1210"].map(usd)
    );
    assert_eq!(
        postings_to(&output, "expenses.other"),
        ["10000", "10300", "10609"].map(usd)
    );
}

#[test]
fn adding_steps_up_by_the_same_amount_in_the_values_unit() {
    let src = "
        let rent {
          jan 2027.. => 1_000/month adding 100/month every year
        }
        let parking {
          jan 2027.. => 200/month adding 10 every year
        }
        entry rent_pay @ 1st {
          assets.cash -> expenses.rent    : rent
          assets.cash -> expenses.parking : parking
        }
    ";
    let output = run(src, &opts("2027-11-01", "2029-01-31")).unwrap();
    assert_eq!(
        postings_to(&output, "expenses.rent"),
        [
            1_000, 1_000, 1_100, 1_100, 1_100, 1_100, 1_100, 1_100, 1_100, 1_100, 1_100, 1_100,
            1_100, 1_100, 1_200
        ]
        .map(Decimal::from)
    );
    assert_eq!(postings_to(&output, "expenses.parking")[2], usd("210"));
}

#[test]
fn each_step_compounds_at_the_rate_on_its_day() {
    let src = "
        let inflation {
          2027 => 10%
          _    => 5%
        }
        let prices {
          2026.. => 1_000 compounding inflation every year
        }
        entry other @ dec 31 { assets.cash -> expenses.other : prices }
    ";
    let output = run(src, &opts("2026-01-01", "2029-12-31")).unwrap();
    assert_eq!(
        postings_to(&output, "expenses.other"),
        ["1000", "1100", "1155", "1212.75"].map(usd)
    );
}

#[test]
fn growth_can_use_any_value() {
    let src = "
        fn half(x) { x / 2 }
        let inflation {
          2027 => 20%
          _    => 10%
        }
        let prices {
          2026.. => 1_000 compounding half(inflation) every year
        }
        entry other @ dec 31 { assets.cash -> expenses.other : prices }
    ";
    let output = run(src, &opts("2026-01-01", "2028-12-31")).unwrap();
    assert_eq!(
        postings_to(&output, "expenses.other"),
        ["1000", "1100", "1155"].map(usd)
    );
}

#[test]
fn growth_errors_point_at_the_offending_text() {
    let cases = [
        (
            "let p {\n _ => 1 compounding 3% every year\n}",
            "needs an arm with a start",
            "compounding 3% every year",
        ),
        (
            "let p {\n may => 1 adding 1 every year\n}",
            "needs an arm with a start",
            "adding 1 every year",
        ),
        (
            "let p {\n 2026.. => 1 compounding 3%/year every year\n}",
            "takes a plain number",
            "3%/year",
        ),
        (
            "let p {\n 2026.. => 1/month adding 1/year every year\n}",
            "`adding` adds an amount per year to an amount per month",
            "1/year",
        ),
        (
            "balances @ 2026-01-01 { assets.a = 1 }\nlet p {\n 2026.. => 1 compounding assets.a every year\n}",
            "values can't read balances",
            "assets.a",
        ),
        (
            "let p {\n 2026.. => 1 compounding 3%\n}",
            "expected `every`",
            "}",
        ),
        (
            "let p {\n 2026.. => 1 adding 1 every fortnight\n}",
            "expected `day`, `week`",
            "fortnight",
        ),
    ];
    for (src, needle, expected) in cases {
        let errors = run(src, &opts("2026-01-01", "2026-01-01")).unwrap_err();
        assert_eq!(highlighted(src, &errors, needle), expected, "{src}");
    }
}

#[test]
fn growth_steps_in_periods_without_a_firing_count_in_each_period() {
    let monthly = format!(
        "
        let fee {{
          2027.. => 100/month adding 10/month every month
        }}
        entry fee_pay @ {QUARTER_ENDS} {{ assets.cash -> expenses.fee : fee }}
    "
    );
    let output = run(&monthly, &opts("2027-01-01", "2027-06-30")).unwrap();
    // 100 + 110 + 120, then 130 + 140 + 150.
    assert_eq!(
        postings_to(&output, "expenses.fee"),
        [330, 420].map(Decimal::from)
    );
    // Yearly steps fall on January 1st, which a quarterly entry's firings
    // never straddle.
    let yearly = monthly.replace("every month", "every year");
    let output = run(&yearly, &opts("2027-01-01", "2028-06-30")).unwrap();
    assert_eq!(
        postings_to(&output, "expenses.fee"),
        [300, 300, 300, 300, 330, 330].map(Decimal::from)
    );
}

#[test]
fn errors_render_without_color_when_asked() {
    let src = "let p = ghost";
    let errors = run(src, &opts("2025-01-01", "2025-01-01")).unwrap_err();
    let plain = saldo::format_errors("test.saldo", src, &errors, false);
    assert!(!plain.contains('\x1b'), "{plain}");
    assert!(saldo::format_errors("test.saldo", src, &errors, true).contains('\x1b'));
}

// --- invariants ---

/// A model exercising most features: warm-up, late openings, values with
/// intervals, tags and totals, paying off balances, functions and schedules.
const KITCHEN_SINK: &str = r#"
    balances @ 2025-01-01 {
      assets.cash       =  5_000
      assets.retirement = 10_000
      liabilities.loan  = 30_000
    }
    balances @ 2026-03-01 { assets.savings = 2_500 }

    let payday = 15th | last
    let interest_rate = 0.05
    let max_401k = 23_000
    let salary {
        2025-01-01..2026-04-16 => 80_000
        2026-04-16..           => 95_000
    }

    fn per_paycheck(annual) { annual / 24 }

    entry paycheck "Paycheck" @ payday {
      let gross = per_paycheck(salary)
      income.salary -> assets.cash       : gross
      assets.cash   -> assets.retirement : min(max_401k - k401[year], gross * 0.15) #k401
    }

    entry accrual "Interest accrual" {
      liabilities.accrued_interest -> expenses.interest : liabilities.loan * interest_rate / 365
    }

    entry loan_payment "Loan payment" @ 17th after accrual {
      assets.cash -> liabilities.accrued_interest : liabilities.accrued_interest
      assets.cash -> liabilities.loan             : 1_000
    }

    entry rent "Rent" @ 2025-01-03.. & fri / 2 {
      assets.cash -> expenses.rent : 1_200
    }

    check paycheck.k401[year] <= max_401k
"#;

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
    std::fs::write(&path, "entry x { income.b -> assets.a : 1 }\n").unwrap();

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

#[test]
fn cli_writes_everything_and_exits_with_3_when_a_check_fails() {
    use std::process::Command;

    let dir = std::env::temp_dir().join(format!("saldo-check-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("model.saldo");
    std::fs::write(
        &path,
        "balances @ 2025-01-01 { assets.cash = 100 }\n\
         entry spend @ last { assets.cash -> expenses.x : 40 }\n\
         check assets.cash >= 0\n",
    )
    .unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_saldo"))
        .arg(&path)
        .args([
            "--from",
            "2025-01-01",
            "--to",
            "2025-12-31",
            "--format",
            "csv",
        ])
        .env("NO_COLOR", "1")
        .output()
        .unwrap();
    std::fs::remove_dir_all(&dir).ok();

    assert_eq!(output.status.code(), Some(3));
    // A header and every day.
    assert_eq!(String::from_utf8_lossy(&output.stdout).lines().count(), 366);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("check failed on 2025-03-31: -20 >= 0, and on 275 more days"));
}

// --- keywords ---

#[test]
fn keywords_are_lowercase_only() {
    // Capitalized keywords are ordinary names.
    let src = "
        let All = 5
        let If = 1
        let Monthly = 1st
        entry gift @ Monthly { income.gifts -> assets.cash : All + If }
    ";
    let output = run(src, &opts("2025-01-01", "2025-01-01")).unwrap();
    assert_eq!(posting(&output, 0, "assets.cash"), Decimal::new(6, 0));

    let errors = run(
        "let p = IF true THEN 1 ELSE 2",
        &opts("2025-01-01", "2025-01-01"),
    );
    assert!(errors.is_err());
}

// --- powers ---

#[test]
fn powers_raise_numbers_exactly_or_as_closely_as_f64_can() {
    let src = "
        let monthly = 1.06^(1/12) - 1
        entry e @ 2026-01-01 {
          income.x -> expenses.a : 1.03^2 * 10_000
          income.x -> expenses.b : -2^2
          income.x -> expenses.c : 2^3^2
          income.x -> expenses.d : 2^-1
          income.x -> expenses.e : monthly * 1_000_000_000
        }
    ";
    let output = run(src, &opts("2026-01-01", "2026-01-01")).unwrap();
    assert_eq!(posting(&output, 0, "expenses.a"), usd("10609"));
    assert_eq!(posting(&output, 0, "expenses.b"), usd("-4"));
    assert_eq!(posting(&output, 0, "expenses.c"), usd("512"));
    assert_eq!(posting(&output, 0, "expenses.d"), usd("0.5"));
    // 1.06^(1/12) - 1 is 0.004867550565343...
    assert_eq!(posting(&output, 0, "expenses.e"), usd("4867550.57"));
}

#[test]
fn powers_take_plain_numbers_and_need_a_value() {
    let errors = |src: &str| run(src, &opts("2026-01-01", "2026-01-31")).unwrap_err();
    assert!(has_error(
        &errors(
            "let salary = 100_000/year
             entry e @ last { income.x -> expenses.a : salary^2 }"
        ),
        "`^` takes plain numbers, like `1.06^(1/12)`, not an amount per year"
    ));
    assert!(has_error(
        &errors("entry e @ last { income.x -> expenses.a : (-8)^(1/3) }"),
        "a negative number has no fractional power"
    ));
    assert!(has_error(
        &errors("entry e @ last { income.x -> expenses.a : 0^-1 }"),
        "division by zero"
    ));
    assert!(has_error(
        &errors("entry e @ last { income.x -> expenses.a : 10^100 }"),
        "arithmetic overflow in `10 ^ 100`"
    ));
}

// --- logical operators ---

fn assert_holds(condition: &str) {
    let src = format!("check {condition}");
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
    let errors = run("check 1 < 2 < 3", &opts("2025-01-01", "2025-01-01")).unwrap_err();
    assert!(has_error(&errors, "comparisons can't be chained"));
}

#[test]
fn logical_operators_require_bools() {
    let errors = run("check 1 and true", &opts("2025-01-01", "2025-01-01")).unwrap_err();
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
            "entry x { income.b -> assets.a : if assets.c > 0 then true else 1 }\nbalances @ 2025-01-01 { assets.c = 1 }",
            "`if` branches must have the same type, but are a bool and a number",
        ),
        (
            "let p = 1 > 0",
            "a value must be a number, but this is a bool",
        ),
        (
            "check 1 + 1",
            "a check must be a bool, but this is a number",
        ),
        ("check 1 == true", "`==` compares a number with a bool"),
        ("check not 1", "the operand of `not` must be a bool"),
        ("let p = -(1 > 0)", "the operand of `-` must be a number"),
        (
            "let p = if 1 then 2 else 3",
            "an `if` condition must be a bool",
        ),
        (
            "let p = max(1 > 0, 2)",
            "an argument to `max` must be a number",
        ),
        (
            "fn positive(x) { x > 0 }\nlet p = positive(1)",
            "a value must be a number, but this is a bool",
        ),
        (
            "entry x {\n let big = 1 > 0\n income.b -> assets.a : big\n}",
            "a `let` must be a number, but this is a bool",
        ),
        (
            "balances @ 2025-01-01 { assets.a = 1 > 0 }",
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
        let p = 5
        check positive(p) and not positive(-p)
    ";
    run(src, &opts("2025-01-01", "2025-01-01")).unwrap();
}

// --- missed total warnings ---

const CAPPED_CONTRIBUTIONS: &str = "
    entry paycheck @ 15th {
      income.salary -> assets.retirement : min(1_000 - k401[year], 300) + k401[quarter] * 0 #k401
    }
    check paycheck.k401[year] <= 1_000
";

fn warnings(src: &str, from: &str) -> Vec<String> {
    let output = run(src, &opts(from, "2025-12-31")).unwrap();
    output.warnings.iter().map(|w| w.message.clone()).collect()
}

#[test]
fn warns_when_a_total_misses_earlier_firings() {
    let output = run(CAPPED_CONTRIBUTIONS, &opts("2025-05-20", "2025-12-31")).unwrap();
    let messages: Vec<&str> = output.warnings.iter().map(|w| w.message.as_str()).collect();
    // One warning per total, even though `paycheck.k401[year]` repeats it.
    assert_eq!(
        messages,
        [
            "`paycheck.k401[year]` is missing amounts from before 2025-05-20",
            "`paycheck.k401[quarter]` is missing amounts from before 2025-05-20",
        ]
    );
    let span = output.warnings[0].span;
    assert_eq!(&CAPPED_CONTRIBUTIONS[span.start..span.end], "k401[year]");
    assert!(
        output.warnings[1].extra[0]
            .1
            .contains("would have fired on 2025-04-15")
    );
}

#[test]
fn warns_when_an_account_total_misses_earlier_firings() {
    let src = "
        entry pay @ 15th { income.salary -> assets.cash : 100 }
        check income.salary[year] >= 0
    ";
    assert_eq!(
        warnings(src, "2025-05-20"),
        ["`income.salary[year]` is missing amounts from before 2025-05-20"]
    );
}

#[test]
fn no_warning_when_nothing_was_missed() {
    // Starting on the first day of the year.
    assert!(warnings(CAPPED_CONTRIBUTIONS, "2025-01-01").is_empty());
    // Starting mid-period, but before the entry first fires.
    assert!(warnings(CAPPED_CONTRIBUTIONS, "2025-01-10").is_empty());
    // Starting mid-year, but an account opening on Jan 1 warms up from there.
    let warmed_up =
        format!("balances @ 2025-01-01 {{ assets.retirement = 0 }}\n{CAPPED_CONTRIBUTIONS}");
    assert!(warnings(&warmed_up, "2025-06-01").is_empty());
}

#[test]
fn unicode_account_names_work_and_align() {
    let src = "
        balances @ 2025-01-01 { assets.geld = 100 }
        entry kaffee { assets.geld -> expenses.café : 3 }
    ";
    let ledger = run(src, &opts("2025-01-01", "2025-01-01"))
        .unwrap()
        .to_ledger();
    assert!(
        ledger.contains("  Expenses:Café   3\n  Assets:Geld    -3\n"),
        "{ledger}"
    );
}

// --- rates ---

#[test]
fn per_year_amounts_add_up_exactly_whatever_the_number_of_paydays() {
    // 2021 starts and ends on a Friday, so it has 27 biweekly paydays; 2022
    // has 26.
    let src = "
        let max_401k = 24_500/year
        entry semi_monthly @ 15th | last {
          income.salary -> assets.retirement.semi_monthly : max_401k
        }
        entry biweekly @ 2021-01-01.. & fri / 2 {
          income.salary -> assets.retirement.biweekly : max_401k
        }
    ";
    let output = run(src, &opts("2021-01-01", "2022-12-31")).unwrap();
    for year in [2021, 2022] {
        for account in [
            "assets.retirement.semi_monthly",
            "assets.retirement.biweekly",
        ] {
            assert_eq!(
                posted_in(&output, account, year),
                usd("24500.00"),
                "{account} {year}"
            );
        }
    }
    let biweekly = postings_to(&output, "assets.retirement.biweekly");
    assert_eq!(biweekly.len(), 27 + 26);
    assert_eq!(biweekly[0], usd("907.41")); // 24_500 / 27
    assert_eq!(biweekly[27], usd("942.31")); // 24_500 / 26
}

#[test]
fn a_raise_applies_from_the_next_paycheck() {
    let src = "
        let salary {
          2026-01-01..2026-07-01 => 100_000/year
          2026-07-01..           => 110_000/year
        }
        entry paycheck @ 15th | last { income.salary -> assets.cash : salary }
    ";
    let output = run(src, &opts("2026-01-01", "2026-12-31")).unwrap();
    let cash = postings_to(&output, "assets.cash");
    assert_eq!(cash[..2], [usd("4166.67"), usd("4166.66")]);
    assert_eq!(cash[12], usd("4583.33")); // 110_000 / 24
    assert_eq!(posted_in(&output, "assets.cash", 2026), usd("105000.00"));
}

#[test]
fn a_limit_minus_its_period_total_is_whats_left_of_it() {
    // Contribute 16% of each paycheck until the yearly limit is reached.
    let src = "
        let salary   = 300_000/year
        let max_401k = 24_500[year]
        entry pay @ 15th | last {
          income.salary -> assets.retirement : min(salary * 0.16, max_401k - contribution[year]) #contribution
        }
        check pay.contribution[year] <= max_401k
    ";
    let output = run(src, &opts("2026-01-01", "2026-12-31")).unwrap();
    let contributions = postings_to(&output, "assets.retirement");
    assert_eq!(
        contributions[..13],
        [[usd("2000.00"); 12].as_slice(), &[usd("500.00")]].concat()
    );
    assert_eq!(
        posted_in(&output, "assets.retirement", 2026),
        usd("24500.00")
    );
}

#[test]
fn interest_accrues_by_the_actual_days_in_the_year() {
    let src = "
        balances @ 2026-01-01 { liabilities.loan = 100_000 }
        let rate = 5%/year
        entry interest {
          liabilities.accrued_interest -> expenses.interest : liabilities.loan * rate
        }
    ";
    let output = run(src, &opts("2026-01-01", "2028-12-31")).unwrap();
    let interest = postings_to(&output, "liabilities.accrued_interest");
    assert_eq!(interest[0], usd("-13.70")); // 5_000 / 365
    // 2028 has 366 days.
    for year in [2026, 2027, 2028] {
        let total = posted_in(&output, "liabilities.accrued_interest", year);
        assert_eq!(total, usd("-5000.00"), "{year}");
    }
}

#[test]
fn a_schedule_starting_midyear_gets_part_of_a_yearly_amount() {
    let src = "
        let salary = 120_000/year
        entry paycheck @ 2026-07-01.. & last { income.salary -> assets.cash : salary }
    ";
    let output = run(src, &opts("2026-01-01", "2026-12-31")).unwrap();
    assert_eq!(postings_to(&output, "assets.cash"), [usd("10000.00"); 6]);
}

#[test]
fn entries_firing_less_often_than_the_rate_collect_the_periods_between() {
    let src = format!(
        "
        let rent    = 1_000/month
        let stipend = 200/week
        entry rent_pay @ {QUARTER_ENDS} {{ assets.cash -> expenses.rent : rent }}
        entry stipend_pay @ 2025-12-26.. & fri / 2 {{ assets.cash -> expenses.stipend : stipend }}
    "
    );
    let output = run(&src, &opts("2026-01-01", "2026-12-31")).unwrap();
    assert_eq!(postings_to(&output, "expenses.rent"), [usd("3000.00"); 4]);
    assert_eq!(
        postings_to(&output, "expenses.stipend"),
        [usd("400.00"); 26]
    );
}

#[test]
fn units_convert_rates_between_months_and_years() {
    let src = r#"
        let salary  = 120_000/year
        let monthly = salary/month
        let raise   = 1_200
        let raised  = (monthly + raise / 12)/year
        entry pay "Pay" @ dec 31 { income.salary -> assets.cash : monthly }
        entry raise_pay "Raise" @ last { income.salary -> assets.cash : raised - salary }
    "#;
    let output = run(src, &opts("2026-01-01", "2026-12-31")).unwrap();
    let pay = |label: &str| {
        let tx = output
            .log
            .transactions
            .iter()
            .position(|t| &*t.label == label);
        posting(&output, tx.unwrap(), "assets.cash")
    };
    assert_eq!(pay("Pay"), usd("120000.00"));
    assert_eq!(pay("Raise"), usd("100.00"));
}

#[test]
fn a_unit_applies_to_what_is_on_its_left() {
    // `130_000 / 52/week` is a weekly rate: the units bind like `/`.
    let src = "
        let salary = 130_000 / 52/week
        entry pay @ sun { income.salary -> assets.cash : salary }
    ";
    let output = run(src, &opts("2026-01-01", "2026-01-31")).unwrap();
    assert_eq!(postings_to(&output, "assets.cash")[1], usd("2500.00"));
}

#[test]
fn functions_pass_rates_through() {
    let src = "
        fn net(gross, rate) { gross - gross * rate }
        let salary = 120_000/year
        entry paycheck @ last { income.salary -> assets.cash : net(salary, 0.25) }
    ";
    let output = run(src, &opts("2026-01-01", "2026-01-31")).unwrap();
    assert_eq!(posting(&output, 0, "assets.cash"), usd("7500.00"));
}

#[test]
fn rates_and_amounts_dont_mix_outside_entries() {
    let cases = [
        (
            "check salary + assets.cash > 0",
            "`+` mixes an amount per year with an amount",
        ),
        (
            "check assets.cash >= salary",
            "`>=` mixes an amount with an amount per year",
        ),
        (
            "let x = salary + rent",
            "`+` mixes an amount per year with an amount per month",
        ),
        (
            "let x = salary * rent",
            "can't multiply an amount per year by an amount per month",
        ),
        (
            "let x = 1 / salary",
            "can't divide a number by an amount per year",
        ),
        (
            "balances @ 2026-01-01 { assets.odd = salary }",
            "an opening balance must be an amount, but this is an amount per year",
        ),
        (
            "let x = salary/week",
            "can't convert an amount per year to per week: \
             a year isn't a fixed number of weeks",
        ),
        (
            "check job.paid[year]/year > 0",
            "`/year` can't apply to a year-to-date total, \
             which is an amount so far, not per period",
        ),
        (
            "fn monthly(x) { x/month }\nlet x = monthly(salary)",
            "can't convert an amount per year to per month inside a function",
        ),
        (
            "let x {\n 2026-01-01..2026-02-01 => salary\n 2026-02-01.. => rent\n}",
            "`x` is an amount per year in one arm but an amount per month in another",
        ),
        (
            "fn f(a, b) { a - b }\ncheck f(salary, assets.cash) > 0",
            "`-` mixes an amount per year with an amount",
        ),
    ];
    for (decl, message) in cases {
        let src = format!(
            "let salary = 120_000/year
             let rent   = 1_000/month
             entry job @ last {{ income.salary -> assets.cash : 1 #paid }}
             {decl}"
        );
        let errors = run(&src, &opts("2026-01-01", "2026-01-31")).unwrap_err();
        assert!(has_error(&errors, message), "{decl}: {errors:?}");
    }
}

#[test]
fn per_day_interest_matches_an_amortization_schedule() {
    // Actual/365 interest on each period's days, rounded once: the first
    // period runs from Jan 10 to Jan 31.
    let src = "
        balances @ 2026-01-10 { liabilities.loan = 10_000 }
        entry loan_payment @ 2026-01-11.. & last {
          let interest = (liabilities.loan * 6% / 365)/day
          liabilities.loan -> expenses.interest : interest
          assets.cash      -> liabilities.loan  : min(300, liabilities.loan + interest)
        }
    ";
    let output = run(src, &opts("2026-01-01", "2029-12-31")).unwrap();
    let expected = "34.52 44.81 48.31 45.51 45.73 43.00 43.12 41.81 39.19 39.17 36.62 36.50 \
                    35.15 30.53 32.43 30.06 29.69 27.40 26.92 25.53 23.36 22.72 20.62 19.89 \
                    18.46 15.93 15.58 13.67 12.67 10.84 9.73 8.25 6.55 5.27 3.65 2.26 0.74";
    let expected: Vec<Decimal> = expected.split_whitespace().map(usd).collect();
    assert_eq!(postings_to(&output, "expenses.interest"), expected);
    let payments = postings_to(&output, "assets.cash");
    assert_eq!(payments.last(), Some(&usd("-146.19")));
    assert_eq!(posted_in(&output, "liabilities.loan", 2029), usd("145.45"));
}

// --- limits and fill ---

#[test]
fn a_limit_is_filled_over_the_firings_left() {
    // A rate is spread over the year, so a job that starts in July
    // contributes half of it; a limit isn't, so it contributes all of it.
    let src = "
        entry rate @ 2026-07-01.. & last {
          income.salary -> assets.rate : 12_000/year
        }
        entry limit @ 2026-07-01.. & last {
          income.salary -> assets.limit : 12_000[year]
        }
    ";
    let output = run(src, &opts("2026-01-01", "2027-12-31")).unwrap();
    assert_eq!(posted_in(&output, "assets.rate", 2026), usd("6000"));
    assert_eq!(posted_in(&output, "assets.limit", 2026), usd("12000"));
    assert_eq!(postings_to(&output, "assets.limit")[0], usd("2000"));
    assert_eq!(posted_in(&output, "assets.limit", 2027), usd("12000"));
    assert_eq!(postings_to(&output, "assets.limit")[6], usd("1000"));
}

#[test]
fn balances_can_say_what_a_tag_has_moved_so_far() {
    // Starting in October, with 18,000 of the year's 24,000 already in.
    let src = "
        let max_401k = 24_000[year]
        entry paycheck @ 15th | last {
          income.salary -> assets.cash       : 5_000
          assets.cash   -> assets.retirement : max_401k #contribution
        }
        balances @ 2026-10-01 {
          assets.cash = 0
          paycheck.contribution[year] = 18_000
        }
        check paycheck.contribution[year] == 24_000 @ 2026-12-31
    ";
    let output = run(src, &opts("2026-10-01", "2027-01-31")).unwrap();
    assert!(output.warnings.is_empty(), "{:?}", output.warnings);
    let contributions = postings_to(&output, "assets.retirement");
    // The 6,000 left over the year's last six paychecks, then a 24th of 2027's.
    assert_eq!(contributions[..6], [usd("1000"); 6]);
    assert_eq!(contributions[6], usd("1000"));
    // Without the total, the whole year's limit goes into the last three
    // months, and saldo warns that it doesn't know what came before.
    let unset = src.replace("paycheck.contribution[year] = 18_000", "");
    let output = run(&unset, &opts("2026-10-01", "2026-10-31")).unwrap();
    assert_eq!(postings_to(&output, "assets.retirement"), [usd("4000"); 2]);
    assert_eq!(
        output.warnings[0].message,
        "what's left here doesn't count what was moved before 2026-10-01"
    );
}

#[test]
fn balances_can_say_what_an_account_has_moved_so_far() {
    let src = "
        entry paycheck @ last { income.salary -> assets.cash : 10_000 }
        balances @ 2026-10-02 {
          income.salary[year] = 90_000
          income.salary[quarter] = 0
        }
        check income.salary[year] == 120_000 and income.salary[quarter] == 30_000 @ 2026-12-31
    ";
    let output = run(src, &opts("2026-10-02", "2026-12-31")).unwrap();
    assert!(output.warnings.is_empty(), "{:?}", output.warnings);
}

#[test]
fn totals_in_balances_must_be_totals() {
    let cases = [
        ("balances @ 2026-01-01 { pay.t[week] = 1 }", "not a week"),
        (
            "balances @ 2026-01-01 { salary[year] = 1 }",
            "`salary` has no total",
        ),
        (
            "balances @ 2026-01-01 { pay.t[year] = 1\n pay.t[year] = 2 }",
            "`pay.t[year]` is already set on 2026-01-01",
        ),
        (
            "balances @ 2026-01-01 { pay.t[year] = salary }",
            "a total must be an amount",
        ),
    ];
    for (decl, message) in cases {
        let src = format!(
            "let salary = 1/year\nentry pay @ last {{ income.a -> assets.b : 1 #t }}\n{decl}"
        );
        let errors = run(&src, &opts("2026-01-01", "2026-01-01")).unwrap_err();
        assert!(has_error(&errors, message), "{decl}: {errors:?}");
    }
}

#[test]
fn a_limit_becomes_a_rate_with_a_unit() {
    let src = "
        let salary   = 120_000/year
        let max_401k = 24_000[year]
        let taxable  = salary - max_401k/year
        entry pay @ last { income.salary -> assets.cash : taxable }
    ";
    let output = run(src, &opts("2026-01-01", "2026-01-31")).unwrap();
    assert_eq!(posting(&output, 0, "assets.cash"), usd("8000"));
    // A limit and a rate don't mix without it.
    let src = src.replace("max_401k/year", "max_401k");
    let errors = run(&src, &opts("2026-01-01", "2026-01-31")).unwrap_err();
    assert!(has_error(
        &errors,
        "`-` mixes an amount per year with an amount each year"
    ));
}

#[test]
fn a_rate_becomes_a_limit_with_a_period() {
    let src = "
        let salary = 10_000/month
        entry pay @ last { income.salary -> assets.cash : salary }
        check income.salary[year] <= salary[year]
        check salary[year] == 120_000
    ";
    run(src, &opts("2026-01-01", "2026-12-31")).unwrap();
    // Without one, it's spread, and doesn't compare with a total.
    let src = "
        let salary = 120_000/year
        entry pay @ last { income.salary -> assets.cash : salary }
        check income.salary[year] <= salary
    ";
    let errors = run(src, &opts("2026-01-01", "2026-12-31")).unwrap_err();
    assert!(has_error(
        &errors,
        "`<=` mixes a year-to-date total with an amount per year"
    ));
    let rendered = saldo::format_errors("test.saldo", src, &errors, false);
    assert!(rendered.contains("write `[year]` after it"), "{rendered}");
}

// --- fill ---

#[test]
fn fill_reaches_a_yearly_target_after_a_midyear_start() {
    let src = r#"
        let salary   = 150_000/year
        let max_401k = 24_500[year]
        let old_contribution {
          2026-01-01..2026-07-01 => 1_000/month
          2026-07-01..           => 0
        }
        entry old_job "Old job" @ last {
          income.old_salary -> assets.retirement : old_contribution #contribution
        }
        entry new_job "New job" @ 2026-07-10.. & fri / 2 {
          income.salary -> assets.cash       : salary
          assets.cash   -> assets.retirement : fill(max_401k - old_job.contribution[year]) #contribution
        }
    "#;
    let output = run(src, &opts("2026-01-01", "2027-12-31")).unwrap();
    let new_job: Vec<Decimal> = output
        .log
        .transactions
        .iter()
        .enumerate()
        .filter(|(_, tx)| &*tx.label == "New job")
        .map(|(i, _)| posting(&output, i, "assets.retirement"))
        .collect();
    // 18_500 left after the old job, over the 13 paydays left in 2026.
    assert_eq!(new_job[0], usd("1423.08"));
    assert_eq!(new_job[..13].iter().sum::<Decimal>(), usd("18500.00"));
    // A full year: 26 paydays.
    assert_eq!(new_job[13], usd("942.31"));
    for year in [2026, 2027] {
        assert_eq!(
            posted_in(&output, "assets.retirement", year),
            usd("24500.00")
        );
    }
    // The salary is still spread, so half a year's worth arrives in 2026.
    assert_eq!(posted_in(&output, "income.salary", 2026), usd("-75000.00"));
}

#[test]
fn fill_makes_up_for_firings_held_back() {
    let src = "
        let cap {
          2026-01-01..2026-07-01 => 500
          2026-07-01..           => 10_000
        }
        entry paycheck @ last {
          income.salary -> assets.retirement : min(fill(24_000[year]), cap)
        }
    ";
    let output = run(src, &opts("2026-01-01", "2026-12-31")).unwrap();
    let contributions = postings_to(&output, "assets.retirement");
    assert_eq!(contributions[..6], [usd("500.00"); 6]);
    // (24_000 - 3_000) / 6 months left
    assert_eq!(contributions[6..], [usd("3500.00"); 6]);
}

#[test]
fn fill_has_to_be_what_a_move_moves() {
    let cases = [
        (
            "let x = fill(max_401k)",
            "`fill` can only be the amount a move moves",
        ),
        (
            "entry a @ last { income.salary -> assets.cash : fill(max_401k) + 100 }",
            "`fill` can only be the amount a move moves",
        ),
        (
            "entry a @ last {\n let f = fill(max_401k)\n income.salary -> assets.cash : f\n}",
            "`fill` can only be the amount a move moves",
        ),
        (
            "entry a @ last { income.salary -> assets.cash : fill(500) }",
            "`fill` needs an amount each period",
        ),
        (
            "entry a @ last { income.salary -> assets.cash : fill(24_500/year) }",
            "but this is an amount per year",
        ),
        (
            "entry a @ last { income.salary -> assets.cash : max_401k + assets.cash }",
            "`+` mixes an amount each year with an amount",
        ),
        (
            "entry a @ last { income.salary -> assets.cash : fill(max_401k - paid[year]) #paid }",
            "`fill` already subtracts what this move has moved",
        ),
        (
            "fn f(x) { fill(x) }\nentry a @ last { income.salary -> assets.cash : f(max_401k) }",
            "`fill` can only be the amount a move moves",
        ),
    ];
    for (decl, message) in cases {
        let src = format!("let max_401k = 24_500[year]\n{decl}");
        let errors = run(&src, &opts("2026-01-01", "2026-01-31")).unwrap_err();
        assert!(has_error(&errors, message), "{decl}: {errors:?}");
    }
}

#[test]
fn fill_warns_when_the_simulation_starts_partway_through_its_period() {
    let src = "
        entry paycheck @ last {
          income.salary -> assets.retirement : 24_000[year]
        }
    ";
    let output = run(src, &opts("2026-03-15", "2026-12-31")).unwrap();
    assert_eq!(output.warnings.len(), 1);
    assert_eq!(
        output.warnings[0].message,
        "what's left here doesn't count what was moved before 2026-03-15"
    );
    let span = output.warnings[0].span;
    assert_eq!(&src[span.start..span.end], "24_000[year]");
    let output = run(src, &opts("2026-01-01", "2026-12-31")).unwrap();
    assert!(output.warnings.is_empty());
}

// --- modules ---

/// Writes `files` into a fresh directory named after `test` and returns it.
fn write_model(test: &str, files: &[(&str, &str)]) -> std::path::PathBuf {
    let dir = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join(test);
    std::fs::remove_dir_all(&dir).ok();
    for (name, text) in files {
        let path = dir.join(name);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }
    dir
}

const LOAN_BALANCES: &str = "
    balances @ 2025-01-01 { liabilities.loan = 10_000 }
";

const LOAN_PAYMENTS: &str = r#"
    let loan_rate = 6%/year
    entry payment "Loan payment" @ last {
      assets.cash -> expenses.interest : liabilities.loan * loan_rate
      assets.cash -> liabilities.loan  : 500
    }
"#;

#[test]
fn modules_in_files_are_like_modules_written_inline() {
    let main = "
        balances @ 2025-01-01 { assets.cash = 20_000 }
        mod loans
        check assets.cash >= 0 and loans.loan_rate > 0
    ";
    // `mod accounts` in `loans.saldo` reads `loans/accounts.saldo`, as Rust
    // does.
    let loans = format!("mod accounts\n{LOAN_PAYMENTS}");
    let dir = write_model(
        "modules_join",
        &[
            ("main.saldo", main),
            ("loans.saldo", &loans),
            ("loans/accounts.saldo", LOAN_BALANCES),
        ],
    );
    let mut sources = saldo::Sources::default();
    let range = opts("2025-01-01", "2025-12-31");
    let output = saldo::run_file(dir.join("main.saldo"), &range, &mut sources).unwrap();

    let single = format!(
        "balances @ 2025-01-01 {{ assets.cash = 20_000 }}
         mod loans {{
           mod accounts {{ {LOAN_BALANCES} }}
           {LOAN_PAYMENTS}
         }}
         check assets.cash >= 0 and loans.loan_rate > 0"
    );
    let expected = run(&single, &range).unwrap();
    assert_eq!(output.to_ledger(), expected.to_ledger());
    assert_eq!(output.to_csv(), expected.to_csv());
    assert!(
        output
            .to_csv()
            .starts_with(r#""date","Assets:Cash","Expenses:Interest","Liabilities:Loan""#)
    );
}

#[test]
fn errors_name_the_file_they_are_in() {
    let dir = write_model(
        "modules_errors",
        &[
            ("main.saldo", "mod loans\nlet a = 1\nlet b = nope"),
            ("loans.saldo", "\n    let c = nada"),
        ],
    );
    let mut sources = saldo::Sources::default();
    let main = dir.join("main.saldo");
    let errors =
        saldo::run_file(&main, &opts("2025-01-01", "2025-01-01"), &mut sources).unwrap_err();
    assert!(has_error(&errors, "unknown name `nope`"));
    let rendered = sources.format_errors(&errors, false);
    let main_name = main.display().to_string();
    let loans_name = dir.join("loans.saldo").display().to_string();
    assert!(rendered.contains(&format!("{main_name}:3:9")), "{rendered}");
    assert!(
        rendered.contains(&format!("{loans_name}:2:13")),
        "{rendered}"
    );
}

#[test]
fn errors_in_module_files_point_into_them() {
    let dir = write_model(
        "modules_parse_error",
        &[
            ("main.saldo", "let a = 1\nmod bad\nmod gone"),
            ("bad.saldo", "let b = 1\nlet p = (1"),
        ],
    );
    let mut sources = saldo::Sources::default();
    let errors = saldo::run_file(
        dir.join("main.saldo"),
        &opts("2025-01-01", "2025-01-01"),
        &mut sources,
    )
    .unwrap_err();
    assert_eq!(errors.len(), 2, "{errors:?}");
    let rendered = sources.format_errors(&errors, false);
    let bad = dir.join("bad.saldo").display().to_string();
    let gone = dir.join("gone.saldo").display().to_string();
    assert!(rendered.contains(&format!("{bad}:2:")), "{rendered}");
    assert!(
        rendered.contains(&format!("could not read `{gone}`")),
        "{rendered}"
    );
    assert!(rendered.contains("main.saldo:3:5"), "{rendered}");
}

#[test]
fn an_unreadable_model_is_a_read_error() {
    let path = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("missing.saldo");
    let mut sources = saldo::Sources::default();
    let errors =
        saldo::run_file(&path, &opts("2025-01-01", "2025-01-01"), &mut sources).unwrap_err();
    assert!(matches!(&errors[0], saldo::Error::Read { path: p, .. } if *p == path));
    let rendered = sources.format_errors(&errors, false);
    assert!(rendered.starts_with(&format!("could not read `{}`", path.display())));
}

#[test]
fn models_given_as_text_cannot_read_module_files() {
    let errors = run("mod loans", &opts("2025-01-01", "2025-01-01")).unwrap_err();
    assert!(has_error(&errors, "only works in a model read from a file"));
}

#[test]
fn a_named_every_nth_counts_from_where_its_used() {
    let src = "
        let biweekly = fri / 2
        entry pay @ 2026-01-02.. & biweekly { income.pay -> assets.cash : 1 }
    ";
    let output = run(src, &opts("2026-01-01", "2026-01-31")).unwrap();
    let dates: Vec<NaiveDate> = output.log.transactions.iter().map(|tx| tx.date).collect();
    assert_eq!(dates, ["2026-01-02", "2026-01-16", "2026-01-30"].map(d));
}

// --- sampling and series ---

#[test]
fn every_reports_the_last_day_of_each_period_and_the_last_day() {
    let src = "entry pay @ 15th { income.salary -> assets.cash : 100 }";
    let output = run(
        src,
        &RunOpts {
            every: saldo::TimeUnit::Month,
            ..opts("2026-01-10", "2026-04-20")
        },
    )
    .unwrap();
    let dates: Vec<NaiveDate> = output.log.snapshots.iter().map(|s| s.date).collect();
    assert_eq!(
        dates,
        ["2026-01-31", "2026-02-28", "2026-03-31", "2026-04-20"].map(d)
    );
    assert!(output.to_csv().ends_with("2026-04-20,400.00,400.00\n"));
}

#[test]
fn series_report_accounts_groups_and_values() {
    let src = "
        let prices { 2026.. => 1 compounding 10% every year }
        let bonus { 2026 => 1_000 }
        balances @ 2026-01-01 {
          assets.cash      = 1_000
          assets.savings   = 500
          liabilities.loan = 300
        }
    ";
    let output = run(
        src,
        &RunOpts {
            series: ["assets.cash", "assets", "liabilities", "prices", "bonus"]
                .map(str::to_string)
                .to_vec(),
            ..opts("2026-12-31", "2027-01-01")
        },
    )
    .unwrap();
    assert_eq!(
        output.to_csv(),
        "\"date\",\"assets.cash\",\"assets\",\"liabilities\",\"prices\",\"bonus\"\n\
         2026-12-31,1000.00,1500.00,300.00,1,1000\n\
         2027-01-01,1000.00,1500.00,300.00,1.1,\n"
    );
}

#[test]
fn an_unknown_series_suggests_a_close_name() {
    let errors = run(
        "let prices = 1\nbalances @ 2026-01-01 { assets.cash = 1 }",
        &RunOpts {
            series: ["assets.csh", "nope"].map(str::to_string).to_vec(),
            ..opts("2026-01-01", "2026-01-01")
        },
    )
    .unwrap_err();
    assert!(matches!(
        &errors[..],
        [
            saldo::Error::UnknownSeries { name: a, close: Some(c) },
            saldo::Error::UnknownSeries { name: b, close: None },
        ] if a == "assets.csh" && c == "assets.cash" && b == "nope"
    ));
}

#[test]
fn cli_rejects_sampling_the_ledger() {
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_saldo"))
        .args(["model.saldo", "--from", "2026-01-01", "--to", "2026-01-31"])
        .args(["--every", "month"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert!(
        String::from_utf8_lossy(&output.stderr)
            .contains("--every and --series pick what CSV reports")
    );
}

// --- random draws ---

fn seeded(from: &str, to: &str, seed: u64) -> RunOpts {
    RunOpts {
        seed,
        ..opts(from, to)
    }
}

/// The standard deviation of `xs`.
fn sd(xs: &[f64]) -> f64 {
    let mean = xs.iter().sum::<f64>() / xs.len() as f64;
    (xs.iter().map(|x| (x - mean).powi(2)).sum::<f64>() / (xs.len() - 1) as f64).sqrt()
}

fn to_f64(x: Decimal) -> f64 {
    x.to_string().parse().unwrap()
}

#[test]
fn a_seed_repeats_a_run_and_another_seed_changes_it() {
    let src = "entry food @ sun { assets.cash -> expenses.food : normal(185, 40) }";
    let run_with = |seed| run(src, &seeded("2026-01-01", "2026-03-31", seed)).unwrap();
    assert_eq!(run_with(7).to_ledger(), run_with(7).to_ledger());
    assert_ne!(run_with(7).to_ledger(), run_with(8).to_ledger());
    assert!(run_with(7).draws);
    let plain = run(
        "entry food @ sun { assets.cash -> expenses.food : 185 }",
        &opts("2026-01-01", "2026-01-31"),
    )
    .unwrap();
    assert!(!plain.draws);
}

#[test]
fn draws_dont_depend_on_where_the_report_starts() {
    let src = "
        balances @ 2026-01-01 { assets.cash = 1_000 }
        entry food @ sun { assets.cash -> expenses.food : normal(185, 40) }
        let rent = normal(2_000, 100)/month
        entry pay_rent @ 1st { assets.cash -> expenses.rent : rent }
    ";
    let whole = run(src, &seeded("2026-01-01", "2026-12-31", 3)).unwrap();
    let late = run(src, &seeded("2026-07-01", "2026-12-31", 3)).unwrap();
    let from_july = |output: &saldo::Output| -> Vec<(NaiveDate, String)> {
        output
            .log
            .transactions
            .iter()
            .filter(|tx| tx.date >= d("2026-07-01") && tx.label.as_ref() != "opening-balances")
            .map(|tx| (tx.date, format!("{:?}", tx.postings)))
            .collect()
    };
    assert_eq!(from_july(&whole), from_july(&late));
}

#[test]
fn a_rate_is_drawn_once_a_period_and_a_number_each_firing() {
    let src = "
        let salary = normal(120_000, 5_000)/year
        entry pay @ 15th | last { income.salary -> assets.cash : salary }
        entry food @ sun { assets.cash -> expenses.food : normal(185, 40) }
    ";
    let output = run(src, &seeded("2026-01-01", "2027-12-31", 1)).unwrap();
    let pay = postings_to(&output, "assets.cash");
    let (first, second) = pay.split_at(pay.len() / 2);
    // One draw a year, split into paychecks that differ by at most a cent.
    for year in [first, second] {
        let year: Vec<&Decimal> = year.iter().filter(|x| **x > Decimal::ZERO).collect();
        assert_eq!(year.len(), 24);
        assert!(year.iter().all(|x| (**x - *year[0]).abs() <= usd("0.01")));
    }
    assert_ne!(
        posted_in(&output, "income.salary", 2026),
        posted_in(&output, "income.salary", 2027)
    );
    let food = postings_to(&output, "expenses.food");
    let distinct: std::collections::HashSet<&Decimal> = food.iter().collect();
    assert_eq!(distinct.len(), food.len());
}

#[test]
fn a_unit_on_an_expression_draws_what_is_written_in_it_once_a_unit() {
    let src = "
        let salary = 100_000/year
        entry pay @ last { income.salary -> assets.cash : salary * normal(1, 10%)/year }
    ";
    let output = run(src, &seeded("2026-01-01", "2026-12-31", 2)).unwrap();
    let pay = postings_to(&output, "assets.cash");
    assert!(
        pay.iter().all(|x| (*x - pay[0]).abs() <= usd("0.01")),
        "{pay:?}"
    );
}

#[test]
fn a_limit_is_drawn_once_a_period_and_filled() {
    let src = "
        let bonus = normal(10_000, 2_000)[year]
        entry pay @ 15th | last { income.bonus -> assets.cash : bonus }
    ";
    let output = run(src, &seeded("2026-01-01", "2027-12-31", 4)).unwrap();
    let year = |y| posted_in(&output, "assets.cash", y);
    assert_ne!(year(2026), year(2027));
    let pay = postings_to(&output, "assets.cash");
    assert!(pay[..24].iter().all(|x| (*x - pay[0]).abs() <= usd("0.01")));
}

#[test]
fn periods_that_roll_into_a_firing_get_their_own_draws() {
    // A monthly entry moving a weekly draw: March 2026 takes five weeks, each
    // with its own draw, so the month's sd is about 40 * sqrt(5), not 5 * 40.
    let src = "entry food @ last { assets.cash -> expenses.food : normal(185, 40)/week }";
    let march: Vec<f64> = (0..300)
        .map(|seed| {
            let output = run(src, &seeded("2026-03-01", "2026-03-31", seed)).unwrap();
            to_f64(postings_to(&output, "expenses.food")[0])
        })
        .collect();
    let found = sd(&march);
    assert!((75.0..105.0).contains(&found), "sd {found}");
}

#[test]
fn naming_a_draw_shares_it() {
    let shared = "
        balances @ 2026-01-01 {
          assets.retirement.a = 1_000
          assets.retirement.b = 1_000
        }
        let market = normal(1%, 4%)
        entry returns @ last {
          for a in assets.retirement { income.investments -> a : a * market }
        }
    ";
    let output = run(shared, &seeded("2026-01-01", "2026-01-31", 5)).unwrap();
    assert_eq!(
        posting(&output, 0, "assets.retirement.a"),
        posting(&output, 0, "assets.retirement.b")
    );
    // Written in the loop, it's drawn for each account.
    let separate = shared.replace("a * market", "a * normal(1%, 4%)");
    let output = run(&separate, &seeded("2026-01-01", "2026-01-31", 5)).unwrap();
    assert_ne!(
        posting(&output, 0, "assets.retirement.a"),
        posting(&output, 0, "assets.retirement.b")
    );
}

#[test]
fn each_module_a_function_makes_draws_for_itself() {
    let src = "
        fn spending(on) {
          mod { entry spend @ last { assets.cash -> on : normal(100, 10) } }
        }
        mod a = spending(on: expenses.a)
        mod b = spending(on: expenses.b)
    ";
    let output = run(src, &seeded("2026-01-01", "2026-01-31", 6)).unwrap();
    assert_ne!(
        postings_to(&output, "expenses.a"),
        postings_to(&output, "expenses.b")
    );
}

#[test]
fn changing_a_distribution_keeps_its_draws_in_step() {
    // The same seed draws the same quantiles, so moving the mean moves every
    // draw by the same amount.
    let src =
        |mean| format!("entry food @ sun {{ assets.cash -> expenses.food : normal({mean}, 40) }}");
    let low = run(&src(185), &seeded("2026-01-01", "2026-03-31", 9)).unwrap();
    let high = run(&src(285), &seeded("2026-01-01", "2026-03-31", 9)).unwrap();
    let (low, high) = (
        postings_to(&low, "expenses.food"),
        postings_to(&high, "expenses.food"),
    );
    assert!(low.iter().zip(&high).all(|(l, h)| *h - *l == usd("100")));
}

#[test]
fn growth_steps_draw_their_own_rates() {
    let src = "
        let prices { 2026.. => 1 compounding normal(3%, 1%) every year }
        entry food @ jan 1 { assets.cash -> expenses.food : 1_000_000 * prices }
    ";
    let output = run(src, &seeded("2026-01-01", "2029-12-31", 1)).unwrap();
    let food: Vec<f64> = postings_to(&output, "expenses.food")
        .into_iter()
        .map(to_f64)
        .collect();
    let growth: Vec<f64> = food.windows(2).map(|w| w[1] / w[0] - 1.0).collect();
    assert_eq!(growth.len(), 3);
    assert!(growth.iter().all(|g| (0.0..0.06).contains(g)), "{growth:?}");
    assert!(growth[0] != growth[1] && growth[1] != growth[2]);
}

#[test]
fn chance_comes_true_about_as_often_as_it_says() {
    let src = "entry repair @ last { assets.cash -> expenses.car : if chance(25%) then 1 else 0 }";
    let output = run(src, &seeded("2000-01-01", "2049-12-31", 1)).unwrap();
    let hits = postings_to(&output, "expenses.car").len();
    // 600 months.
    assert!((120..180).contains(&hits), "{hits}");
}

#[test]
fn uniform_draws_between_its_bounds() {
    let src = "entry e @ * { assets.cash -> expenses.x : uniform(300, 2_500) }";
    let output = run(src, &seeded("2026-01-01", "2026-12-31", 1)).unwrap();
    let xs = postings_to(&output, "expenses.x");
    assert!(xs.iter().all(|x| *x >= usd("300") && *x <= usd("2500")));
}

#[test]
fn draws_cant_be_written_where_they_would_be_shared_or_facts() {
    let errors = |src: &str| run(src, &opts("2026-01-01", "2026-01-31")).unwrap_err();
    assert!(has_error(
        &errors("check normal(1, 1) > 0"),
        "a check can't draw at random"
    ));
    assert!(has_error(
        &errors("balances @ 2026-01-01 { assets.cash = normal(1, 1) }"),
        "an opening balance can't be drawn at random"
    ));
    assert!(has_error(
        &errors(
            "entry e @ last { income.x -> assets.cash : 1 #x }
             balances @ 2026-01-01 { e.x[year] = uniform(1, 2) }"
        ),
        "a total so far can't be drawn at random"
    ));
    assert!(has_error(
        &errors("fn f(x) { x * normal(1, 0.1) }"),
        "function `f` can't draw at random"
    ));
    assert!(has_error(
        &errors(
            "fn loan(rate) { mod { entry pay @ last { assets.cash -> expenses.i : rate } } }
             mod car = loan(rate: normal(5, 1))"
        ),
        "`loan` copies `rate` wherever it uses it"
    ));
    assert!(has_error(
        &errors(
            "fn loan(rate) {
               let r = rate * normal(1, 0.1)
               mod { entry pay @ last { assets.cash -> expenses.i : r } }
             }"
        ),
        "`r` is copied wherever the module of `loan` uses it"
    ));
}

#[test]
fn a_rate_cant_hold_a_draw_that_changes_more_often() {
    let errors = |src: &str| run(src, &opts("2026-01-01", "2026-01-31")).unwrap_err();
    let message = "this is an amount per month, but part of it is drawn anew every day";
    assert!(has_error(
        &errors(
            "let x = normal(100, 10)
             entry food { assets.cash -> expenses.food : x/month }"
        ),
        message
    ));
    assert!(has_error(
        &errors("entry e @ last { assets.cash -> expenses.x : 500/month + normal(10, 1) }"),
        message
    ));
    assert!(has_error(
        &errors("let r = normal(800, 100)/week\nlet m = r/month"),
        "can't convert"
    ));
    assert!(has_error(
        &errors("let r = normal(800, 100)/month\nlet y = r/year"),
        "this is an amount per year, but part of it is drawn anew every month"
    ));
}

#[test]
fn draws_take_arguments_of_one_kind() {
    let errors = |src: &str| run(src, &opts("2026-01-01", "2026-01-31")).unwrap_err();
    assert!(has_error(
        &errors("entry e @ last { assets.cash -> expenses.x : normal(10/month, 1/year) }"),
        "`normal` mixes an amount per month with an amount per year"
    ));
    assert!(has_error(
        &errors("entry e @ last { assets.cash -> expenses.x : if chance(4%/year) then 1 else 0 }"),
        "`chance` takes a plain probability"
    ));
    assert!(has_error(
        &errors("entry e @ last { assets.cash -> expenses.x : normal(1, 1) and true }"),
        "an operand of `and`"
    ));
}

#[test]
fn bad_arguments_are_errors_that_name_the_seed() {
    let errors = |src: &str| run(src, &seeded("2026-01-01", "2026-01-31", 12)).unwrap_err();
    assert!(has_error(
        &errors("entry e @ last { assets.cash -> expenses.x : normal(10, -1) }"),
        "a standard deviation can't be negative, but this is -1 (drawing with seed 12)"
    ));
    assert!(has_error(
        &errors("entry e @ last { assets.cash -> expenses.x : uniform(2, 1) }"),
        "`uniform` draws from 2 up to 1"
    ));
    assert!(has_error(
        &errors("entry e @ last { assets.cash -> expenses.x : if chance(150%) then 1 else 0 }"),
        "a chance is between 0% and 100%"
    ));
}

#[test]
fn a_draw_cant_change_during_periods_that_roll_into_an_amount_that_reads_balances() {
    let src = "
        balances @ 2026-01-01 { liabilities.loan = 10_000 }
        entry interest @ 2026-01-31.. & last / 3 {
          liabilities.loan -> expenses.interest : liabilities.loan * normal(0.5%, 0.1%)/month
        }
    ";
    let errors = run(src, &opts("2026-01-01", "2026-12-31")).unwrap_err();
    assert!(has_error(
        &errors,
        "this draws anew during the months before 2026-04-01"
    ));
    // Drawn once a year, it doesn't change between quarterly firings' months.
    let yearly = src.replace("normal(0.5%, 0.1%)/month", "normal(6%, 1%)/year");
    run(&yearly, &opts("2026-01-01", "2026-12-31")).unwrap();
}

#[test]
fn cli_prints_a_random_seed_and_takes_one() {
    use std::process::Command;

    let dir = std::env::temp_dir().join(format!("saldo-seed-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("model.saldo");
    std::fs::write(
        &path,
        "entry food @ sun { assets.cash -> expenses.food : normal(185, 40) }\n",
    )
    .unwrap();
    let saldo = |extra: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_saldo"))
            .arg(&path)
            .args(["--from", "2026-01-01", "--to", "2026-01-31"])
            .args(extra)
            .output()
            .unwrap()
    };
    let random = saldo(&[]);
    let stderr = String::from_utf8_lossy(&random.stderr).into_owned();
    let seed = stderr
        .strip_prefix("note: drew with seed ")
        .and_then(|rest| rest.split(';').next())
        .unwrap_or_else(|| panic!("{stderr}"));
    let repeated = saldo(&["--seed", seed]);
    std::fs::remove_dir_all(&dir).ok();

    assert_eq!(random.stdout, repeated.stdout);
    // Given a seed, it doesn't print it.
    assert!(repeated.stderr.is_empty());
}
