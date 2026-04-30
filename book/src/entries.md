# Entries

An entry (also called a *flow*) is a transaction that fires on a schedule.
Each time it fires, it moves money between accounts according to a list of
postings. All postings in one firing must balance to zero.

## Syntax

```
entry <schedule> "<label>" {
  <account> [= <amount>] [as <leg>]
  ...
} [as <alias>]
```

## Postings

Each line inside the braces is a posting: an account and an optional amount.

### Fixed amount

```
Assets:Cash = 5_000
Expenses:Rent = 3_915.30
```

The account balance is increased by the given amount. Use a negative
expression to decrease a balance.

### Auto-balance

Omit `=` on exactly one posting per entry. saldo calculates the amount that
makes all postings sum to zero:

```
entry monthly "Jim's paycheck" {
  Assets:Retirement:Jim = 1_500
  Assets:Cash           = 7_500
  Income:Gross:Salary:Jim          // auto-balanced: receives -9_000
}
```

Because income accounts carry a negative balance by convention, the
auto-balanced posting receives the negation of the net inflow.

### Clearing a balance (`all`)

Use `= all` to move the entire current balance of an account:

```
entry monthly "Loan payment" {
  Liabilities:AccruedInterest = all   // clears whatever has accrued
  Liabilities:Loan            = 2_000
  Assets:Cash
}
```

## Named legs

Append `as <name>` to a posting to give it a leg name. The leg accumulates
into period-to-date totals that can be read in later expressions within the
same simulation day:

```
entry semi_monthly "Seb's paycheck" {
  Assets:Retirement:Seb = min(max_401k - retirement_contribution.ytd,
                              max_401k / 24 + 0.01)  as retirement_contribution
  Assets:Cash           = seb_salary / 24 - retirement_contribution
  Income:Gross:Salary:Seb                            as gross_income
} as seb_paycheck
```

`retirement_contribution.ytd` is the running year-to-date sum of every
`retirement_contribution` leg across all firings so far this year. Once a
leg name is established, you can reference it in the same entry on
subsequent posting lines (as `retirement_contribution` above, without `.ytd`),
which gives you the value from the *current* firing.

Available aggregation suffixes:

| Suffix | Resets |
|--------|--------|
| `.ytd` | January 1 |
| `.qtd` | First day of each quarter |
| `.mtd` | First day of each month |

## Flow alias

The optional `as <alias>` at the end of the block gives the flow a name for
use in scoped aggregations:

```
} as seb_paycheck
```

Reference a leg scoped to this flow with `<alias>.<leg>.ytd`:

```
assert seb_paycheck.retirement_contribution.ytd <= 24_500
```

Without an alias, leg aggregations are unscoped and can be referenced
directly by leg name.

## Complete example

```
param max_401k       : usd/year = 24_500
param jim_salary     : usd/year {
  from 2025-12-31 to 2026-04-01 = 115_000
  from 2026-04-01               = 130_000
}
param retirement_rate = 0.16

entry monthly "Jim's paycheck" {
  Assets:Retirement:Jim = min(max_401k - retirement_contribution.ytd,
                              jim_salary * retirement_rate / 12)  as retirement_contribution
  Assets:Cash           = jim_salary / 12 - retirement_contribution
  Income:Gross:Salary:Jim
} as jim_paycheck

entry daily "Interest accrual" {
  Liabilities:AccruedInterest = Liabilities:Loan * interest_rate / 365
  Expenses:Interest
}
```
