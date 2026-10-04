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

The expression is evaluated at the start of each simulated day and again
after each entry fires. For expressions built from numbers and other constant
params, the value never changes. A param that reads an account balance follows
that balance as it changes.

## Time-varying params

A param can take different values on different days. Write its *arms* in
braces, each a condition and a value:

```
param <name> {
  <condition> = <expression>
  <condition> = <expression>
  ...
}
```

```
param salary {
  from 2025-12-31 to 2026-04-01 = 115_000 per year
  from 2026-04-01               = 130_000 per year
}
```

On each day, the first arm whose condition covers that day gives the value.
So put an exception above the arms it's an exception to:

```
param daycare {
  in jan 2027     = 0      // closed for renovation
  in jul, aug     = 0      // summer break
  from 2027-09-01 = 2_100
  else            = 1_950
}
```

### Conditions

| Condition | Covers |
|-----------|--------|
| `from 2026-04-01` | that day and every day after |
| `from 2026-01-01 to 2026-04-01` | from the first date up to, but not including, the second |
| `from 2027`, `from jul 2027`, `from q3 2027` | the start of that year, month or quarter and every day after |
| `in may` | May, every year |
| `in q2` | April through June, every year |
| `in jul through apr` | July through April, every year |
| `in 2026` | all of 2026 |
| `in mar 2028`, `in q2 2028` | that month or quarter, once |
| `in 2026 through 2028` | the start of 2026 to the end of 2028 |
| `else` | every day |

`in` takes a list, like `in jul, aug` or `in q2, dec 2028`, and covers the
days of any of them. A month or quarter without a year comes every year, and
one with a year comes once. Both ends of a `through` need a year, or neither.
Write years in full.

An arm that the arms above it already cover entirely, like one after `else`,
is an error, since it would never apply.

On a day that no arm covers, the param has no value. Using it on such a day is
an error, so end with `else`, or add an arm with the value you want, like
`= 0`, for those days.

A param's arms must agree on what kind of value it is, like a rate per year
(see [Rates](#rates)). A rate in an arm for some months is still a rate:
`in jul, aug = 60_000 per year` posts a twelfth of 60,000 in each of July and
August from a monthly entry, not half of it. For what a month costs, write
`per month` or a plain amount.

A more complete example:

```
param beth_salary {
  in 2026 = 160_000 per year
  in 2027 = 190_000 per year
  in 2028 = 225_000 per year
  in 2029 = 255_000 per year
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
