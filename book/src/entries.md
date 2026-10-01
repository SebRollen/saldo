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

Amounts are rounded to cents using round-half-to-even (`0.125` becomes
`0.12`), the same rule hledger and beancount use. If every posting in a
firing comes to zero, no transaction is written for it.

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
param jim_salary = 120_000 per year
param max_401k   = 24_500 per year

entry semi_monthly "Jim's paycheck" {
  Assets:Retirement:Jim = min(jim_salary * 0.16,
                              max_401k - retirement_contribution.ytd)  as retirement_contribution
  Assets:Cash           = jim_salary - retirement_contribution
  Income:Gross:Salary:Jim                                              as gross_income
} as jim_paycheck
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
} as jim_paycheck
```

Reference a leg scoped to this flow with `<alias>.<leg>.ytd`:

```
assert that jim_paycheck.retirement_contribution.ytd <= 24_500
```

Without an alias, a leg can only be referenced from inside its own entry
(`retirement_contribution.ytd`). Add an alias to read it from other entries,
params, or assertions.

## Rates

A posting whose amount is a rate, like `24_500 per year` (see
[Params](./params.md#rates)), posts each firing's share of it:

```
param salary   = 120_000 per year
param max_401k = 24_500 per year

entry every month on the 15th and last day "Paycheck" {
  Assets:Retirement = max_401k               as contribution
  Assets:Cash       = salary - contribution
  Income:Salary
}
```

The days in each calendar year that fit the entry's schedule split the
year's amount equally, rounded so that they add up to it exactly. Here
that's 24 contributions of 1020.83 or 1020.84, which come to 24,500.00. An
`every second friday` schedule has 26 paydays in some years and 27 in
others, and each year still comes to 24,500.00.

In a posting, a rate used with an amount, like `salary - contribution`,
means the firing's share of it, and so do rates that `min`, `max` and `if`
choose between. A rate added to, subtracted from or compared with a total
over the same period counts in full, so this contributes 16% of each
paycheck until the year's limit is reached:

```
entry every month on the 15th and last day "Paycheck" {
  Assets:Retirement = min(salary * 0.16, max_401k - contribution.ytd) as contribution
  Assets:Cash       = salary - contribution
  Income:Salary
}
```

In detail:

- **Periods are calendar periods** of the rate: years, quarters,
  months, weeks (Monday to Sunday) or days. A daily entry posts 1/365 of a
  yearly rate, or 1/366 in a leap year, so interest at
  `Liabilities:Loan * rate` accrues by the actual number of days.
- **Changes apply from the next firing.** Each firing uses the rate's value
  on its own day, so a raise on April 1 shows up in the next paycheck.
- **The schedule's pattern decides the split, not its `from` date.** An
  entry `every month from 2026-07-01` posts a twelfth of a yearly rate each
  month, so half of it in 2026. Firings before the simulation starts count
  too, so you see the same paychecks whatever `--from` you choose. To post
  the whole amount over the firings that are left, use
  [`fill`](#filling-a-target).
- **A period without a firing rolls into the next one.** A `quarterly` entry
  posts three months of a rate per month, and an `every second friday`
  entry posts two weeks of a rate per week.

The last rule gives you the other common way of paying a yearly salary
biweekly: the same amount every payday, so that a year with 27 paydays pays
more. Declare the salary per week:

```
param salary = (130_000 / 52) per week
```

### Filling a target

A rate spreads evenly, like a salary. Some amounts are targets instead: you
want a year's 401(k) contributions to reach the limit, however many
paychecks are left. `fill` posts what's left of the period's amount,
divided by the firings left in the period:

```
param salary   = 150_000 per year
param max_401k = 24_500 per year

entry every second friday from 2026-07-10 "New job" {
  Assets:Retirement = fill(max_401k) as contribution
  Assets:Cash       = salary - contribution
  Income:Salary
}
```

The job starts in July, so its 13 paydays in 2026 contribute 24,500 between
them, while the salary, which is spread, pays half a year. From 2027 there
are 26 paydays, and each contributes a 26th.

What's left is the amount minus what the posting has already posted this
period, so if an earlier firing posts less, later ones make up the
difference. `min(fill(max_401k), cap)` contributes as much as `cap` allows
and catches up when it can. To count contributions from elsewhere, subtract
them from the target, for example `fill(max_401k - old_job.contribution.ytd)`.
Don't subtract the posting's own total: `fill` already does.

The period comes from the target: a rate's unit, or a total's period, like
the year of a `.ytd`. `fill` can only be the amount a posting posts, or
what `min`, `max` or `if` choose for it. Like a `.ytd` total, it only knows
what was posted since the simulation started, so saldo warns if it starts
partway through a period after the entry would have fired.

## Complete example

```
param max_401k   = 24_500 per year
param jim_salary {
  from 2025-12-31 to 2026-04-01 = 115_000 per year
  from 2026-04-01               = 130_000 per year
}
param retirement_rate = 16%
param interest_rate   = 5% per year

entry monthly "Jim's paycheck" {
  Assets:Retirement:Jim = min(jim_salary * retirement_rate,
                              max_401k - retirement_contribution.ytd)  as retirement_contribution
  Assets:Cash           = jim_salary - retirement_contribution
  Income:Gross:Salary:Jim
} as jim_paycheck

entry daily "Interest accrual" {
  Liabilities:AccruedInterest = Liabilities:Loan * interest_rate
  Expenses:Interest
}
```
