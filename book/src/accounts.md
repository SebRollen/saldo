# Accounts

An account is a named balance that the simulator tracks over time. Every
account that appears in an entry or assertion must be declared.

## Syntax

```
account <path> [= <expression> @ <date>]
```

The path is one or more identifiers joined by colons:

```
account Assets:Cash
account Assets:Retirement:Jim
account Liabilities:Loan
account Income:Gross:Salary:Jim
account Expenses:Rent
```

## Opening balance

Without `= … @ …`, an account starts at zero and is available for the entire
simulation. Supply an initial value and a date to give the account a known
opening balance on a specific day:

```
account Assets:Cash             = 12_500    @ 2025-01-01
account Liabilities:Loan        = -450_000  @ 2025-01-01
account Assets:Retirement:Seb   = 87_340.22 @ 2024-07-01
```

The `= value` and `@ date` must always appear together — one without the other
is a parse error.

The expression is evaluated on the opening date. It can reference params and
accounts that are already open, but not accounts that open later or leg
aggregations.

### Referencing accounts before their opening date

Referencing an account (in an entry posting or an expression) before its
`@ date` is a runtime error:

```
account Assets:Cash = 1000 @ 2025-06-01

// This entry fires in January — before Assets:Cash opens — and will error:
entry monthly "Paycheck" {
  Assets:Cash = 500
  Income:Salary
}
```

To avoid the error, ensure entry schedules start no earlier than the latest
opening date of any account they reference, or set the opening date early
enough to cover the simulation range.

### Simulation start after opening date

If your simulation start is later than an account's opening date, saldo
automatically warms up the simulation from the opening date. All entries that
fire during the warm-up period are processed normally — only their output
(ledger transactions, CSV rows) is suppressed. The opening-balances entry in
ledger output reflects the actual balance at your simulation start, after the
warm-up.

```
account Assets:Cash = 1000 @ 2024-01-01

entry monthly "Paycheck" {
  Assets:Cash = 500
  Income:Salary
}

// Running from 2025-01-01: Assets:Cash opens at 1000 on 2024-01-01, then
// 12 monthly paycheck entries fire during warm-up, so the opening balance
// shown in the ledger output is 7000 (1000 + 12 × 500).
```

## Naming conventions

saldo does not enforce any particular hierarchy, but the double-entry
conventions used throughout these docs are:

| Root | Holds |
|------|-------|
| `Assets:…` | Things you own (cash, retirement, savings) |
| `Liabilities:…` | Things you owe (loans, accrued interest) |
| `Income:…` | Sources of income — carried as negative by convention |
| `Expenses:…` | Spending categories |

Income accounts are negative because every paycheck entry credits income
(negative posting) and debits assets (positive posting), keeping the net
of all postings at zero.

## Using accounts in expressions

Reference an account by its full path in any expression:

```
// Current balance of a liability account
Liabilities:Loan * interest_rate / 365

// Assert cash never goes below a threshold
assert that Assets:Cash >= 10_000

// Compute daily interest on the outstanding balance
entry daily "Interest accrual" {
  Liabilities:AccruedInterest = Liabilities:Loan * interest_rate / 365
  Expenses:Interest
}
```

The value of an account reference is the balance at the start of the current
simulation day, before any entries fire on that day. Within a single entry the
balance reflects each posting as it is applied, so a later posting in the same
entry sees an updated value.

## Declaration order

Accounts appear in the CSV output columns in the order they are declared.
Declare them in the order you want to read them.

## Complete example

```
account Assets:Cash             = 12_500   @ 2025-01-01
account Assets:Retirement:Jim   = 45_000   @ 2025-01-01
account Liabilities:Loan        = -320_000 @ 2025-01-01
account Income:Gross:Salary:Jim
account Expenses:Rent

param jim_salary     : usd/year = 130_000
param interest_rate             = 0.065

entry monthly "Jim's paycheck" {
  Assets:Cash           = jim_salary / 12
  Income:Gross:Salary:Jim
}

entry monthly "Rent" {
  Expenses:Rent = 3_915
  Assets:Cash
}

entry daily "Loan interest" {
  Liabilities:Loan = Liabilities:Loan * interest_rate / 365
  Expenses:Interest
}

assert that Assets:Cash >= 0
```
