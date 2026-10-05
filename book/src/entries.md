# Entries

An entry moves money between accounts on a [set of days](./schedules.md).
Each time it fires, it makes one transaction.

## Syntax

```
entry <name> ["<label>"] [@ <days>] {
  let <name> = <expression>
  <from> -> <to> : <amount> [#<tag>]
  ...
}
```

```
entry rent "Rent" @ 1st {
  assets.cash -> expenses.rent : 3_000
}
```

The label is what the transaction is called in ledger output; without one, it's
the entry's name, in full if it's in a module, like `loans.repay`. Without
`@`, the entry fires every day.

## Moves

Each line `from -> to : amount` moves an amount from one account to another.
Every move balances by itself, so every transaction does too. An entry can
move money through an account, and its transaction shows what each account
gets in the end:

```
let salary = 120_000/year
let max_401k = 24_000/year

entry paycheck "Paycheck" @ last {
  income.salary -> assets.cash       : salary
  assets.cash   -> assets.retirement : max_401k
  assets.cash   -> expenses.taxes    : salary * 25%
}
```

```
2026-01-31 Paycheck
  Assets:Cash            5500
  Income:Salary        -10000
  Assets:Retirement      2000
  Expenses:Taxes         2500
```

Money moves from income into what you own, from what you own into what you
spend or owe, and so on. A negative amount moves money the other way.

Amounts are rounded to cents using round-half-to-even (`0.125` becomes
`0.12`), the same rule hledger and beancount use. If an entry moves no money
on a day, no transaction is written for it.

### Clearing a balance

Since accounts read positive for their kind (see
[Accounts](./accounts.md#kinds)), moving an account's balance clears it:

```
entry loan_payment "Loan payment" @ 17th {
  assets.cash -> liabilities.accrued_interest : liabilities.accrued_interest
  assets.cash -> liabilities.loan             : 2_000
}
```

## Reading balances

Every move in an entry is worked out from the balances before the entry fired,
so one move never sees another move of the same entry. Entries that fire on
the same day fire in the order they're written, so a later entry sees what
earlier ones moved.

## `let`

`let` names a value for the rest of the entry, worked out each time it fires.
Unlike a [value](./values.md), it can read balances and totals:

```
let rate = 6%/year

entry repay "Loan payment" @ last {
  let interest = liabilities.loan * rate
  liabilities.loan -> expenses.interest : interest
  assets.cash      -> liabilities.loan  : min(600, liabilities.loan + interest)
}
```

## Tags and totals

`#name` after a move tags it. `[year]`, `[quarter]` or `[month]` after a tag
gives what its moves moved so far this period:

```
let salary = 300_000/year
let max_401k = 24_500/year

entry paycheck "Paycheck" @ 15th | last {
  income.salary -> assets.cash       : salary
  assets.cash   -> assets.retirement : min(salary * 16%, max_401k - contribution[year]) #contribution
}

check paycheck.contribution[year] <= max_401k
```

Inside the entry, a tag is named alone; elsewhere, it's named after its entry,
like `paycheck.contribution`. A total counts every firing so far this period,
including earlier ones today but not the firing in progress. Totals reset at
the start of each year, quarter or month.

## Rates

A move whose amount is a rate, like `24_500/year` (see
[Values](./values.md#rates)), moves each firing's share of it:

```
let salary   = 120_000/year
let max_401k = 24_500/year

entry paycheck "Paycheck" @ 15th | last {
  income.salary -> assets.cash       : salary
  assets.cash   -> assets.retirement : max_401k
}
```

The days in each calendar year that fit the entry's days split the year's
amount equally, rounded so that they add up to it exactly. Here that's 24
contributions of 1020.83 or 1020.84, which come to 24,500.00. An entry on
`2026-01-02.. & fri / 2` has 26 paydays in some years and 27 in others, and
each year still comes to 24,500.00.

In an entry, a rate used with an amount, like `salary - assets.cash`, means
the firing's share of it, and so do rates that `min`, `max` and `if` choose
between. A rate added to, subtracted from or compared with a total over the
same period counts in full, so this contributes 16% of each paycheck until
the year's limit is reached:

```
entry paycheck "Paycheck" @ 15th | last {
  income.salary -> assets.retirement : min(salary * 0.16, max_401k - contribution[year]) #contribution
}
```

In detail:

- **Periods are calendar periods** of the rate: years, quarters, months, weeks
  (Monday to Sunday) or days. An entry that fires every day moves 1/365 of a
  yearly rate, or 1/366 in a leap year, so interest at
  `liabilities.loan * rate` accrues by the actual number of days.
- **Changes apply from the next firing.** Each firing uses the rate's value on
  its own day, so a raise on April 1 shows up in the next paycheck.
- **The entry's days decide the split, not when it starts.** An entry on
  `2026-07-01.. & last` moves a twelfth of a yearly rate each month, so half of
  it in 2026. Firings before the simulation starts count too, so you see the
  same paychecks whatever `--from` you choose. To move the whole amount over
  the firings that are left, use [`fill`](#filling-a-target).
- **A period without a firing rolls into the next one,** back to when the
  entry starts. A quarterly entry moves three months of a rate per month, an
  entry on `2026-01-02.. & fri / 2` moves two weeks of a rate per week, and an
  entry on `2026-03-17.. & last` first moves March 17 to 31 of a rate per day.
  Months the entry's days leave out don't roll in (see
  [Sets of days](./schedules.md#keeping-some-months)). Each period that rolls
  in gets its own amount, from its last day, so a raise in February is in a
  quarterly entry's February. A rate that reads a balance, like
  `liabilities.loan * interest`, only has today's value, though, so if a
  value it reads changes during those periods, that's an error: fire the entry
  at least once a period.

The last rule gives you the other common way of paying a yearly salary
biweekly: the same amount every payday, so that a year with 27 paydays pays
more. Declare the salary per week, and start the entry on an earlier payday so
that the first one in your simulation covers two weeks:

```
let salary = 130_000 / 52/week
```

It also charges interest by the actual days in each period. This loan starts
on January 10 and is paid at the end of each month, with interest on each
period's days at 1/365 of the yearly rate:

```
entry loan_payment "Loan payment" @ 2026-01-11.. & last {
  let interest = (liabilities.loan * 6% / 365)/day
  liabilities.loan -> expenses.interest : interest
  assets.cash      -> liabilities.loan  : min(300, liabilities.loan + interest)
}
```

The first payment covers January 11 to 31, and `min` makes the last one
smaller, paying off what's left.

### Filling a target

A rate spreads evenly, like a salary. Some amounts are targets instead: you
want a year's 401(k) contributions to reach the limit, however many paychecks
are left. `fill` moves what's left of the period's amount, divided by the
firings left in the period:

```
let salary   = 150_000/year
let max_401k = 24_500/year

entry new_job "New job" @ 2026-07-10.. & fri / 2 {
  income.salary -> assets.cash       : salary
  assets.cash   -> assets.retirement : fill(max_401k) #contribution
}
```

The job starts in July, so its 13 paydays in 2026 contribute 24,500 between
them, while the salary, which is spread, pays half a year. From 2027 there are
26 paydays, and each contributes a 26th.

What's left is the amount minus what the move has already moved this period,
so if an earlier firing moves less, later ones make up the difference.
`min(fill(max_401k), cap)` contributes as much as `cap` allows and catches up
when it can. To count contributions from elsewhere, subtract them from the
target, for example `fill(max_401k - old_job.contribution[year])`. Don't
subtract the move's own total: `fill` already does.

The period comes from the target: a rate's unit, or a total's period, like the
year of a `[year]`. `fill` can only be the amount a move moves, or what `min`,
`max` or `if` choose for it. Like a `[year]` total, it only knows what was
moved since the simulation started, so saldo warns if it starts partway through
a period after the entry would have fired.
