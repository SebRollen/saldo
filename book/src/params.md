# Params

A param is a named numeric value that can change over time. Params let
you express things like salary, contribution limits, or interest rates
in one place and reference them throughout your entries and assertions.

## Constant params

```
param <name> = <expression>
```

```
param interest_rate = 5%
param retirement_rate = 0.16
param max_401k = 24_500 per year
```

A `%` after a value divides it by 100, so `5%` is `0.05`. `per year` makes
the value a rate (see [Rates](#rates)).

The expression is re-evaluated at the start of each simulated day. For
expressions built from numbers and other constant params, the value never
changes. A param that reads an account balance follows that balance as it
changes.

## Time-varying params

```
param <name> {
  from <date> [to <date>] = <expression>
  from <date> [to <date>] = <expression>
  ...
}
```

```
param salary {
  from 2025-12-31 to 2026-04-01 = 115_000 per year
  from 2026-04-01               = 130_000 per year
}
```

Each interval specifies a `from` date (inclusive) and an optional `to`
date (exclusive). The simulator uses whichever interval covers the current
day. Intervals must not overlap. An interval without a `to` clause extends
indefinitely.

On a day that no interval covers (before the first interval, in a gap
between intervals, or after the last one ends), the param has no value.
Using it on such a day is an error, so add an interval with the value you
want, for example `= 0`, to cover those days.

A more complete example:

```
param beth_salary {
  from 2026-01-01 to 2027-01-01 = 160_000 per year
  from 2027-01-01 to 2028-01-01 = 190_000 per year
  from 2028-01-01 to 2029-01-01 = 225_000 per year
  from 2029-01-01 to 2030-01-01 = 255_000 per year
}
```

## Rates

`per day`, `per week`, `per month`, `per quarter` or `per year` after a
value makes it a *rate*: an amount per period, like a salary or a yearly
contribution limit. Write a rate the way you'd say it, and let entries work
out how much of it each firing posts (see [Entries](./entries.md#rates)):

```
param salary   = 120_000 per year
param rent     = 3_000 per month
param interest = 5% per year
```

`per` binds tighter than any other operator, so `salary - 500 per month`
subtracts 500 a month, and `(a + b) per year` needs its parentheses.

saldo checks how rates combine before it simulates anything:

| Expression | Is |
|------------|----|
| `salary * 0.16`, `salary / 2` | a rate per year |
| `salary + bonus` (both per year) | a rate per year |
| `salary + 5_000` | a rate per year: plain numbers take on the unit of what they're combined with |
| `Liabilities:Loan * interest` | a rate per year |
| `rent per year` | a rate per year: 36,000 |
| `max_401k - contribution.ytd` | an amount: what's left of this year's limit |
| `salary - Assets:Cash` | an error, except in a posting |
| `salary + rent` | an error, except in a posting |

A rate and a total over the same period, like a yearly limit and a `.ytd`
total, can be added, subtracted and compared: the rate counts in full. Other
amounts, like account balances, only mix with rates in an entry's postings,
where a rate means the firing's share of it.

A param takes on the kind of its value, so `param gross = salary + bonus` is
a rate too. `per` converts rates between months, quarters and years, or
between days and weeks, but not from weeks or days to months or years, since
those aren't a fixed number of weeks or days.

## Using params in expressions

Reference a param by name in any expression:

```
jim_salary * 0.16
Liabilities:Loan * interest_rate
min(salary * rate, max_401k - retirement_contribution.ytd)
```

A time-varying param automatically returns the right value for the current
date, so you never need to branch on time in your expressions.

## Aggregations

Named legs on entries (see [Entries](./entries.md)) accumulate into
period-to-date buckets that you can read in any expression:

| Syntax | Meaning |
|--------|---------|
| `<leg>.ytd` | Year-to-date total of the named leg |
| `<leg>.qtd` | Quarter-to-date total |
| `<leg>.mtd` | Month-to-date total |
| `<flow>.<leg>.ytd` | Year-to-date total, scoped to a specific flow alias |

These reset automatically at the start of each year, quarter, or month.

```
min(salary * 0.16, max_401k - retirement_contribution.ytd)
```
