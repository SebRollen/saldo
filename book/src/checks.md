# Checks

A check is a condition that must hold on certain days. Checks are how you
write down financial constraints and goals.

A failing check doesn't stop the simulation: saldo writes all its output,
then reports each check that failed, on the first day it did and how many
days it failed on, and exits with status 3:

```
Error: check failed on 2027-07-01: -1371.42 >= 0, and on 152 more days, last on 2027-12-10
```

Errors in the model exit with status 1, so scripts can tell the two apart.
Only the days from `--from` count (see [How a simulation
runs](./simulation.md#warm-up)).

## Syntax

```
check <condition> [@ <days>]
```

Without `@`, a check is checked every day. With a [set of
days](./schedules.md), it's checked only on those days.

## Every day

```
check assets.cash >= 0
check paycheck.contribution[year] <= 24_500
```

The first makes sure cash never goes negative. The second makes sure a tag's
total never goes over a limit.

## On some days

```
check assets.retirement >= 0 @ mar 31 | jun 30 | sep 30 | dec 31
check assets.cash >= 10_000 @ last
check liabilities.accrued_interest >= 0 @ fri
check assets.retirement.beth == 24_500 @ 2026-12-31
```

## Conditions

Conditions use the same operators as other expressions:

| Operator | Meaning |
|----------|---------|
| `<`, `<=` | Less than, at most |
| `>`, `>=` | Greater than, at least |
| `==`, `!=` | Equal, not equal |
| `and`, `or`, `not` | Combine conditions (`not` binds tightest, then `and`, then `or`) |
| `if … then … else …` | Conditional (both `then` and `else` are required) |

Accounts, groups of accounts, values and totals all work in checks.

Comparisons can't be chained: write `0 <= x and x <= 100`, not
`0 <= x <= 100`. The right side of `and` and `or` is only evaluated when
needed.

## Examples

```
// Cash never goes negative
check assets.cash >= 0

// The 401(k) limit isn't breached
let max_401k = 24_500[year]
check paycheck.contribution[year] <= max_401k

// A retirement target is hit by a date
check assets.retirement.beth >= 24_500 @ 2026-12-31

// Cash stays within a band
check assets.cash >= 1_000 and assets.cash <= 50_000

// Net worth stays positive at the end of each year
check assets - liabilities > 0 @ dec 31
```
