# Params

A param is a named numeric value that can change over time. Params let
you express things like salary, contribution limits, or interest rates
in one place and reference them throughout your entries and assertions.

## Constant params

```
param <name> [: <unit>] = <expression>
```

```
param interest_rate = 0.05
param retirement_rate = 0.16
param max_401k : usd/year = 24_500
```

The expression is evaluated once and the param holds that value for the
entire simulation.

## Time-varying params

```
param <name> [: <unit>] {
  from <date> [to <date>] = <expression>
  from <date> [to <date>] = <expression>
  ...
}
```

```
param salary : usd/year {
  from 2025-12-31 to 2026-04-01 = 115_000
  from 2026-04-01               = 130_000
}
```

Each interval specifies a `from` date (inclusive) and an optional `to`
date (exclusive). The simulator uses whichever interval covers the current
day. Intervals must not overlap. An interval without a `to` clause extends
indefinitely.

A more complete example:

```
param beth_salary : usd/year {
  from 2026-01-01 to 2027-01-01 = 160_000
  from 2027-01-01 to 2028-01-01 = 190_000
  from 2028-01-01 to 2029-01-01 = 225_000
  from 2029-01-01 to 2030-01-01 = 255_000
}
```

## Units

The optional `: <unit>` annotation is documentation — it is not enforced
by the simulator. Units help readers understand what a number represents.

```
param max_401k   : usd/year = 24_500
param hsa_limit  : usd      = 8_550
param rate       : %        = 0.05
```

A unit is a single identifier (`usd`, `%`, `year`) or two identifiers
separated by `/` (`usd/year`).

## Using params in expressions

Reference a param by name in any expression:

```
jim_salary / 12
interest_rate / 365
min(max_401k - retirement_contribution.ytd, salary * rate / 12)
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
min(max_401k - retirement_contribution.ytd, max_401k / 24)
```
