# saldo

A small domain-specific language for personal financial simulation. You
describe accounts, parameters, and recurring cash flows in a plain-text
file, then ask saldo to simulate them over a date range and emit
transactions. The default output is plain-text accounting transactions
that can be imported into tools like ledger/hledger for reporting and
analysis.

## Example

```
account Assets:Cash       =   5_000 @ 2026-01-01
account Liabilities:Loan  = -30_000 @ 2026-01-01
account Liabilities:AccruedInterest
account Income:Salary
account Expenses:Interest

schedule paycheck_schedule = monthly on the 15th and last
param interest_rate = 5% per year
param salary {
    from 2025-01-01 to 2025-04-16 = 80_000 per year
    from 2025-04-16               = 95_000 per year // promoted!
}

entry paycheck_schedule "Paycheck" {
  Assets:Cash = salary
  Income:Salary
}

entry daily "Interest accrual" {
  Liabilities:AccruedInterest = Liabilities:Loan * interest_rate
  Expenses:Interest
}

entry monthly on the 17th "Loan payment" {
  Liabilities:AccruedInterest = all
  Liabilities:Loan            = 2_000
  Assets:Cash
}
assert daily that Assets:Cash >= 0
```

When run through the `saldo` CLI, this file generates transactions:
```
❯ saldo budget.saldo --from 2026-01-01 --to 2027-01-01
2026-01-01 opening-balances
  Assets:Cash               5000
  Liabilities:Loan        -30000
  Equity:OpeningBalances   25000

2026-01-01 Interest accrual
  Liabilities:AccruedInterest  -4.11
  Expenses:Interest             4.11

2026-01-02 Interest accrual
  Liabilities:AccruedInterest  -4.11
  Expenses:Interest             4.11
…
2026-01-15 Paycheck
  Assets:Cash     3958.33
  Income:Salary  -3958.33
…
2026-01-17 Loan payment
  Liabilities:AccruedInterest     69.86
  Liabilities:Loan                 2000
  Assets:Cash                  -2069.86

2026-01-18 Interest accrual
  Liabilities:AccruedInterest  -3.83
  Expenses:Interest             3.83
…
```

The transactions can be piped into other PTA tools for reporting:
```
> saldo budget.saldo --from 2026-01-01 --to 2027-01-01 | hledger -f - bal
            75108.23  Assets:Cash
            25000.00  Equity:OpeningBalances
              904.10  Expenses:Interest
           -95000.00  Income:Salary
              -12.33  Liabilities:AccruedInterest
            -6000.00  Liabilities:Loan
--------------------
                   0
```


## Usage

```
saldo <path> --from YYYY-MM-DD --to YYYY-MM-DD [--format ledger|csv]
```

- `--from` / `--to` — inclusive date range to simulate
- `--format ledger` (default) — outputs double-entry ledger transactions
- `--format csv` — outputs a daily balance sheet as CSV

## Documentation

The full language reference is available at https://sebrollen.github.io/saldo/

## Language summary

### Accounts

```
account Assets:Cash = 5_000 @ 2026-01-01
account Liabilities:Loan
```

Accounts hold balances (stocks). Names are colon-separated paths. An optional `= <expr> @ <date>` sets the opening balance on that date; accounts without one start at zero.

### Parameters

```
param interest_rate = 0.05

param jim_salary {
  from 2025-12-31 to 2026-04-01 = 115_000 per year
  from 2026-04-01               = 130_000 per year
}
```

Parameters are named scalars used inside flow expressions. A parameter can be a constant or a date-scheduled value that changes over time. `per year` (or `per day`, `week`, `month`, `quarter`) after a value makes it a rate: entries spread it over their firings, so each year's paychecks add up to exactly the yearly salary.

### Entries

```
entry monthly "Jim's paycheck" {
  Assets:Retirement:Jim = min(jim_salary * retirement_rate, max_401k - retirement_contribution.ytd)  as retirement_contribution
  Assets:Cash           = jim_salary - retirement_contribution
  Income:Gross:Salary:Jim
} as jim_paycheck
```

An entry fires on a schedule and posts amounts to accounts. Every entry is a double-entry transaction: if one posting has no amount, it auto-balances to the negation of the sum of the other postings.

The string label (e.g. `"Jim's paycheck"`) is mandatory and appears in ledger output. The `as <ident>` alias is optional; add it when you need to reference the entry's named legs from other entries (e.g. `jim_paycheck.retirement_contribution.ytd`).

**Schedules:**

Schedules determine when assertions and entries are triggered. They can
be simple, like `daily` which triggers every day, or complex like
`every 3rd month on the 17th and last day from 2024-01-01`. Schedules
can be declared using the `schedule` keyword, or built inline.

**Named legs** (`as <name>`) make a posting's value referenceable inside the same flow and via period aggregates.

**Period aggregates:** `<leg>.ytd`, `<leg>.qtd`, `<leg>.mtd` accumulate a named leg's value year-to-date, quarter-to-date, or month-to-date. Cross-flow access uses `<flow-alias>.<leg>.ytd`.

**Special amount `all`:** zeroes out the account (posts the negation of its current balance).

### Assertions

```
assert that Assets:Cash >= 0
assert 2026-12-31 that Assets:Retirement:Beth == 24_500
```

Assertions are checked after flows run each day. Simulation aborts with an error if any assertion fails.

### Expressions

| Form | Description |
|------|-------------|
| `1_000`, `0.05` | Numeric literals (underscores ignored) |
| `6.2%` | Percentage: divides by 100 |
| `24_500 per year` | Rate: an amount per `day`, `week`, `month`, `quarter` or `year` |
| `true`, `false` | Boolean literals |
| `Assets:Cash` | Account or parameter reference |
| `retirement_contribution` | Named leg reference (within its flow) |
| `leg.ytd` / `leg.qtd` / `leg.mtd` | Period aggregate |
| `alias.leg.ytd` | Cross-flow period aggregate |
| `a + b`, `a - b`, `a * b`, `a / b` | Arithmetic |
| `a == b`, `a != b`, `a < b`, `a <= b`, `a > b`, `a >= b` | Comparison (can't be chained) |
| `a and b`, `a or b`, `not a` | Logical operators |
| `if c then a else b` | Conditional |
| `min(a, b)`, `max(a, b)` | Built-in functions |
| `abs(x)`, `floor(x)`, `ceil(x)`, `round(x)` | Built-in functions |
| `net(gross, 0.3)` | User-defined function call |

Values are numbers or bools. saldo checks that each is used where it's
expected (amounts and params are numbers, assertions and `if` conditions are
bools) before it simulates anything.

Posting amounts and opening balances are rounded to cents, and `round(x)`
rounds to a whole number. Both use round-half-to-even (`round(2.5) == 2`),
as hledger and beancount do. Entries whose postings all come to zero are left
out of the output.

### Functions

```
fn net(gross, rate) {
  let tax = gross * rate;
  gross - tax
}
```

Functions are pure: they can only use their parameters and local `let`
bindings, and may not recurse. The final expression is the return value.

## Building

```
cargo build --release
```

Requires Rust 1.91+.

## Tree-sitter grammar

The `treesitter/` directory contains a Tree-sitter grammar for the saldo language, with bindings for Rust, Node.js, Python, Go, Swift, and C.
