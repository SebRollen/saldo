# saldo

A small domain-specific language for personal financial simulation. You
describe accounts, parameters, and recurring cash flows in a plain-text
file, then ask saldo to simulate them over a date range and emit
transactions. The default output is plain-text accounting transactions
that can be imported into tools like ledger/hledger for reporting and
analysis.

## Example

```
account Assets:Cash       =   5_000
account Liabilities:Loan  = -30_000
account Liabilities:AccruedInterest
account Income:Salary
account Expenses:Interest

schedule paycheck_schedule = monthly on the 15th and last
param interest_rate = 0.05
param salary {
    from 2025-01-01 to 2025-04-16 = 80_000
    from 2025-04-16               = 95_000 // promoted!
}

entry paycheck_schedule "Paycheck" {
  Assets:Cash = salary / 24
  Income:Salary
}

entry daily "Interest accrual" {
  Liabilities:AccruedInterest = Liabilities:Loan * interest_rate / 365
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
  Assets:Cash              5000
  Liabilities:Loan       -30000
  Equity:OpeningBalances  25000

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
  Liabilities:AccruedInterest  69.87
  Liabilities:Loan           2000
  Assets:Cash               -2069.87

2026-01-18 Interest accrual
  Liabilities:AccruedInterest  -3.84
  Expenses:Interest             3.84
…
```

The transactions can be piped into other PTA tools for reporting:
```
> saldo budget.saldo --from 2026-01-01 --to 2027-01-01 | hledger -f - bal
            75107.92  Assets:Cash
            25000.00  Equity:OpeningBalances
              904.30  Expenses:Interest
           -94999.92  Income:Salary
              -12.30  Liabilities:AccruedInterest
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
account Assets:Cash = 5_000
account Liabilities:Loan
```

Accounts hold balances (stocks). Names are colon-separated paths. An optional `= <expr>` sets the opening balance.

### Parameters

```
param interest_rate = 0.05

param jim_salary : usd/year {
  from 2025-12-31 to 2026-04-01 = 115_000
  from 2026-04-01               = 130_000
}
```

Parameters are named scalars used inside flow expressions. A parameter can be a constant or a date-scheduled value that changes over time. The optional `: unit` annotation is documentation only.

### Entries

```
entry monthly "Jim's paycheck" {
  Assets:Retirement:Jim = min(max_401k - retirement_contribution.ytd, jim_salary * retirement_rate / 12)  as retirement_contribution
  Assets:Cash           = jim_salary / 12 - retirement_contribution
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
assert on 2026-12-31 that Assets:Retirement:Beth == 24_500
```

Assertions are checked after flows run each day. Simulation aborts with an error if any assertion fails.

### Expressions

| Form | Description |
|------|-------------|
| `1_000`, `0.05` | Numeric literals (underscores ignored) |
| `true`, `false` | Boolean literals |
| `Assets:Cash` | Account or parameter reference |
| `retirement_contribution` | Named leg reference (within its flow) |
| `leg.ytd` / `leg.qtd` / `leg.mtd` | Period aggregate |
| `alias.leg.ytd` | Cross-flow period aggregate |
| `a + b`, `a - b`, `a * b`, `a / b` | Arithmetic |
| `a == b`, `a < b`, `a <= b`, `a > b`, `a >= b` | Comparison |
| `if c then a else b` | Conditional |
| `min(a, b)`, `max(a, b)` | Built-in functions |
| `abs(x)`, `floor(x)`, `ceil(x)`, `round(x)` | Built-in functions |

## Building

```
cargo build --release
```

Requires Rust 2024 edition (Rust 1.85+).

## Tree-sitter grammar

The `treesitter/` directory contains a Tree-sitter grammar for the saldo language, with bindings for Rust, Node.js, Python, Go, Swift, and C.
