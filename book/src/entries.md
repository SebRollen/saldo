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
let max_401k = 24_000[year]

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
entry loan_payment "Loan payment" @ 17th after accrual {
  assets.cash -> liabilities.accrued_interest : liabilities.accrued_interest
  assets.cash -> liabilities.loan             : 2_000
}
```

## Reading balances

Every move in an entry is worked out from the balances before the entry fired,
so one move never sees another move of the same entry.

### Which fires first

On a day several entries fire, an entry that reads an account, or a total,
fires after the entries that move money into or out of it, so it sees what
they moved. A savings sweep that reads `assets.cash` fires after the paycheck
that moves money into it, wherever each is written:

```
entry sweep @ last {
  assets.cash -> assets.savings : assets.cash - 2_000
}

entry paycheck @ 15th | last {
  income.salary -> assets.cash : 4_000
}
```

Entries that don't read what the others move fire in the order they're
written.

Two entries can each read what the other moves. Here the interest accrual
reads the loan, which the payment moves, and the payment reads the accrued
interest, which the accrual moves. Which goes first changes the result, so
saldo asks you to choose with `after`:

```
entry accrual "Interest accrual" {
  liabilities.accrued_interest -> expenses.interest : liabilities.loan * 5%/year
}

entry loan_payment "Loan payment" @ 17th after accrual {
  assets.cash -> liabilities.accrued_interest : liabilities.accrued_interest
  assets.cash -> liabilities.loan             : 2_000
}
```

`after` takes a list of entries, by their full names, like `after paycheck,
loans.car`. It only matters on days both fire.

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

Like a value, a `let` can be a bool, like `let short = assets.cash < 1_000`,
to use in an `if` below. So can a `let` in a [function](./fns.md).

## Choosing moves with `if`

`if` picks which moves an entry makes each time it fires, from the balances
and values that day:

```
entry sweep "Savings sweep" @ last {
  if assets.cash > 20_000 {
    assets.cash -> assets.savings : assets.cash - 20_000
  } else if assets.cash < 5_000 {
    assets.savings -> assets.cash : 1_000
  }
}
```

The first branch whose condition holds runs, or the `else` branch, if there
is one. A branch can hold any statements, including `let`s, which are only in
scope in it. A rate moved in a branch that doesn't run moves nothing that day:
its share isn't carried to the next firing. A limit catches up on later
firings.

## Going through accounts with `for`

`for` makes the same moves for each account under a group, with a name
standing for the account:

```
let monthly_return = 0.5%

entry returns "Investment returns" @ last {
  for a in assets.retirement, assets.hsa {
    income.investments -> a : a * monthly_return
  }
}
```

Each account under `assets.retirement`, and `assets.hsa`, gets its own return,
all in one transaction. The name reads the account's balance, can be moved
into or out of, and has totals, like `a[year]`. A new account under
`assets.retirement` is included without changing the entry.

## Tags and totals

`#name` after a move tags it. `[year]`, `[quarter]` or `[month]` after a tag
gives what its moves moved so far this period:

```
let salary = 300_000/year
let max_401k = 24_500[year]

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

A move whose amount is a rate, like `120_000/year` (see
[Values](./values.md#rates)), moves each firing's share of it:

```
let salary = 120_000/year

entry paycheck "Paycheck" @ 15th | last {
  income.salary -> assets.cash : salary
}
```

The days in each calendar year that fit the entry's days split the year's
amount equally, rounded so that they add up to it exactly. Here that's 24
paychecks of 5,000. An entry on `2026-01-02.. & fri / 2` has 26 paydays in
some years and 27 in others, and each year still comes to 120,000.00.

In an entry, a rate used with an amount, like `salary - assets.cash`, means
the firing's share of it, and so do rates that `min`, `max` and `if` choose
between. So this contributes 16% of each paycheck until what's left of the
year's limit is less:

```
let max_401k = 24_500[year]

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
  the firings that are left, use a [limit](#limits).
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

## Limits

A rate spreads evenly, like a salary. Some amounts are limits instead, like
`24_500[year]` (see [Values](./values.md#limits)): you want a year's 401(k)
contributions to reach the limit, however many paychecks are left. A move
whose amount is a limit moves what's left of it this period, divided by the
firings left in the period:

```
let salary   = 150_000/year
let max_401k = 24_500[year]

entry new_job "New job" @ 2026-07-10.. & fri / 2 {
  income.salary -> assets.cash       : salary
  assets.cash   -> assets.retirement : max_401k #contribution
}
```

The job starts in July, so its 13 paydays in 2026 contribute 24,500 between
them, while the salary, which is spread, pays half a year. From 2027 there are
26 paydays, and each contributes a 26th.

What's left is the limit minus what's been moved toward it this period: the
total of the move's tag, if it has one, or else what the move has moved. So if
an earlier firing moves less, later ones make up the difference, and a model
that starts partway through a year can say what was contributed before it in a
`balances` block, like `new_job.contribution[year] = 6_000` (see
[Accounts](./accounts.md#totals-so-far)). A limit
can be all of what a move moves, or what `min`, `max` or `if` pick for it,
like `max(max_401k, 0)`. Like a `[year]` total, it only knows what was moved
since the simulation started, so saldo warns if it starts partway through a
period after the entry would have fired.

### Filling a target

`fill` moves what's left of any target, the same way: a limit, or a total like
what's left of a limit after contributions elsewhere:

```
entry new_job "New job" @ 2026-07-10.. & fri / 2 {
  assets.cash -> assets.retirement : fill(max_401k - old_job.contribution[year])
}
```

Don't subtract the move's own total: `fill` already does. A plain number
combined with a limit becomes a limit too, so `min(max_401k, 500)` is a limit
of 500 a year; to cap each firing at 500 and catch up when it can, fill the
limit first: `min(fill(max_401k), 500)`.
