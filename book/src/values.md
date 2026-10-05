# Values

A value is a named number that can change over time. Values let you write
things like a salary, a contribution limit or an interest rate in one place,
and use them throughout your entries and checks.

## Constant values

```
let <name> = <expression>
```

```
let interest_rate = 5%
let retirement_rate = 0.16
let salary = 120_000/year
let max_401k = 24_500[year]
```

A `%` after a number divides it by 100, so `5%` is `0.05`. `/year` makes the
value a rate (see [Rates](#rates)), and `[year]` a limit (see
[Limits](#limits)).

A value depends on nothing but the date: it can use numbers, other values and
functions, but not account balances or totals, which change as entries fire.
That way saldo can work a value out for any day. To work something out from a
balance, use a `let` in an entry (see [Entries](./entries.md#let)) or write it
in a check.

## Values that change over time

A value can take different values on different days. Write its *arms* in
braces, each a [set of days](./schedules.md) and a value:

```
let <name> {
  <days> => <expression>
  <days> => <expression>
  ...
}
```

```
let salary {
  2025-12-31..2026-04-01 => 115_000/year
  2026-04-01..           => 130_000/year
}
```

On each day, the first arm whose days include it gives the value, and `_`
covers every day. So put an exception above the arms it's an exception to:

```
let daycare {
  jan 2027      => 0      // closed for renovation
  jul | aug     => 0      // summer break
  2027-09-01..  => 2_100
  _             => 1_950
}
```

An arm that the arms above it already cover entirely, like one after `_`, is
an error, since it would never apply.

On a day that no arm covers, the value has no value. Using it on such a day is
an error, so end with `_`, or add an arm with the value you want, like `=> 0`,
for those days.

A value's arms must agree on what kind of value it is, like a rate per year
(see [Rates](#rates)). A rate in an arm for some months is still a rate:
`jul | aug => 60_000/year` posts a twelfth of 60,000 in each of July and August
from a monthly entry, not half of it. For what a month costs, write `/month`
or a plain amount. To spread a yearly amount over just some months, leave the
others out of the entry's days instead: `@ 1st & sep..=jun` splits
`24_000/year` into ten firings of 2,400 (see
[Sets of days](./schedules.md#keeping-some-months)).

An entry reads a value on the day it fires, so that's the day an arm's days
are checked, whatever the money is for. Rent for September paid on August 31
needs an arm that covers August 31.

When an entry moves a rate for periods that have no firing of their own, like
a quarterly entry moving an amount per month, each of those periods gets its
own value, from its last day. A quarterly bill of `power` above moves
January's, February's and March's amounts together at the end of March.

A more complete example:

```
let beth_salary {
  2026 => 160_000/year
  2027 => 190_000/year
  2028 => 225_000/year
  2029 => 255_000/year
}
```

### Growth

An arm's value can grow from the arm's start, a step at a time:

```
let inflation = 3%

let prices {
  2026.. => 1 compounding inflation every year      // 1, 1.03, 1.0609…
}

let tuition {
  jul 2026.. => 1_950 compounding 4% every year     // up 4% each July 1st
}

let rent {
  jan 2027.. => 3_000/month adding 100/month every year
}
```

`compounding` multiplies the value by one plus its rate at each step, and
`adding` adds its amount. `every` says how often it steps, a `day`, `week`,
`month`, `quarter` or `year` at a time from the arm's first day, so
`jul 2026..` steps up every July 1st. Only an arm whose days have a start can
grow: one with a year, like `2026..` or `jul 2027`.

`compounding` takes a plain number, like `4%`. What `adding` adds is the same
kind as the value, like an amount `/month`, or a plain number, which takes on
the value's kind.

Each step uses the rate or amount on the day it's taken, so a rate that changes
over time compounds each year at that year's rate.

A value that grows from 1, like `prices` above, is an index: multiply other
amounts by it, like `80_000/year * prices`, to have them rise with it.

## Rates

`/day`, `/week`, `/month`, `/quarter` or `/year` after a value makes it a
*rate*: an amount per period, like a salary, which accrues over the period.
Write a rate the way you'd say it, and let entries work out how much of it
each firing moves (see [Entries](./entries.md#rates)):

```
let salary   = 120_000/year
let rent     = 3_000/month
let interest = 5%/year
```

A unit binds like any other `/`, to what's on its left, so
`salary - 500/month` subtracts 500 a month, `130_000 / 52/week` is a weekly
rate, and `(a + b)/year` needs its parentheses.

saldo checks how rates combine before it simulates anything:

| Expression | Is |
|------------|----|
| `salary * 0.16`, `salary / 2` | a rate per year |
| `salary + bonus` (both per year) | a rate per year |
| `salary + 5_000` | a rate per year: plain numbers take on the unit of what they're combined with |
| `liabilities.loan * interest` | a rate per year |
| `rent/year` | a rate per year: 36,000 |
| `salary - assets.cash` | an error, except in an entry |
| `salary + rent` | an error, except in an entry |
| `salary - income.salary[year]` | an error: a rate accrues, so it doesn't compare with what's moved so far |

Amounts, like account balances, only mix with rates in entries, where a rate
means the firing's share of it.

A value takes on the kind of its expression, so `let gross = salary + bonus`
is a rate too. A unit converts rates between months, quarters and years, or
between days and weeks, but not from weeks or days to months or years, since
those aren't a fixed number of weeks or days.

## Limits

`[day]`, `[week]`, `[month]`, `[quarter]` or `[year]` after a value makes it a
*limit*: an amount each period, which doesn't accrue. A contribution limit is
the same whenever in the year you start contributing:

```
let max_401k = 24_500[year]
let max_hsa  = 4_300[year]
```

An entry that moves a limit moves what's left of it this period, divided by
the firings left in the period (see [Entries](./entries.md#limits)). Where a
rate pays half a year's salary to a job that starts in July, a limit
contributes the whole year's 24,500 over its paydays.

A limit and a total over the same period are both amounts in the current
period, so they combine:

| Expression | Is |
|------------|----|
| `max_401k - contribution[year]` | an amount: what's left of this year's limit |
| `contribution[year] <= max_401k` | a check that the limit hasn't been passed |
| `max_401k * prices`, `max_401k / 2` | a limit each year |
| `salary[year]` | a limit each year: a rate's amount for the whole year |

## Using values

Use a value by its name in any expression:

```
let jim_salary = 130_000/year
let interest_rate = 6%/year
let max_401k = 24_500[year]

entry paycheck @ last {
  income.salary -> assets.cash : jim_salary - jim_salary * 0.16
}

entry interest {
  liabilities.accrued -> expenses.interest : liabilities.loan * interest_rate
}
```

A value that changes over time automatically has the right value for the
current date, so you never need to branch on time in your expressions. A
value in a [module](./modules.md) is named by its full path, like
`loans.rate`.
