# saldo

A small domain-specific language for personal financial simulation. You describe accounts, parameters, and recurring cash flows in a plain-text file, then ask saldo to simulate them over a date range and emit transactions.

## Example

```
account Assets:Cash       =   5_000
account Liabilities:Loan  = -30_000
account Expenses:Interest

param interest_rate = 0.05

daily interest_accrual {
  Liabilities:AccruedInterest = Liabilities:Loan * interest_rate / 365
  Expenses:Interest
}

monthly loan_payment {
  Liabilities:AccruedInterest = all
  Liabilities:Loan            = 2_000
  Assets:Cash
}

assert Assets:Cash >= 0
```

## Usage

```
saldo <path> --from YYYY-MM-DD --to YYYY-MM-DD [--format ledger|csv]
```

- `--from` / `--to` — inclusive date range to simulate
- `--format ledger` (default) — outputs double-entry ledger transactions
- `--format csv` — outputs a daily balance sheet as CSV

## Language reference

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

### Flows

```
monthly jim_paycheck {
  Assets:Retirement:Jim = min(max_401k - retirement_contribution.ytd, jim_salary * retirement_rate / 12)  as retirement_contribution
  Assets:Cash           = jim_salary / 12 - retirement_contribution
  Income:Gross:Salary:Jim
}
```

A flow fires on a schedule and posts amounts to accounts. Every flow is a double-entry transaction: if one posting has no amount, it auto-balances to the negation of the sum of the other postings.

**Schedules:** `daily`, `monthly`, `quarterly`, `yearly`, `on YYYY-MM-DD`

- `monthly` fires on the last day of each month
- `quarterly` fires on the last day of March, June, September, and December
- `yearly` fires on December 31

**Named legs** (`as <name>`) make a posting's value referenceable inside the same flow and via period aggregates.

**Period aggregates:** `<leg>.ytd`, `<leg>.qtd`, `<leg>.mtd` accumulate a named leg's value year-to-date, quarter-to-date, or month-to-date. Cross-flow access uses `<flow>.<leg>.ytd`.

**Special amount `all`:** zeroes out the account (posts the negation of its current balance).

### Assertions

```
assert Assets:Cash >= 0
assert on 2026-12-31 Assets:Retirement:Beth == 24_500
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
| `flow.leg.ytd` | Cross-flow period aggregate |
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
