# Accounts

An account is a named balance that the simulator tracks over time. Every
account that appears in an entry or assertion must be declared.

## Syntax

```
account <path> [= <expression>]
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

Without `=`, an account starts at zero. Supply an expression to give it an
opening balance:

```
account Assets:Cash             = 12_500
account Liabilities:Loan        = -450_000
account Assets:Retirement:Seb   = 87_340.22
```

The expression is evaluated once, before the simulation begins. It can
reference params but not other accounts or leg aggregations.

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
account Assets:Cash             = 12_500
account Assets:Retirement:Jim   = 45_000
account Liabilities:Loan        = -320_000
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
