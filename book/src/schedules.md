# Sets of days

A set of days says when something happens: when an entry fires, when a check
is checked, or which days a value's arm covers. Sets of days are expressions:
literals combined with operators.

```
entry paycheck @ 15th | last { income.salary -> assets.cash : 4_000 }

entry daycare @ 1st & sep..=jun { assets.cash -> expenses.daycare : 2_000 }

check assets.cash >= 10_000 @ last

let payday = 2026-01-02.. & fri / 2
```

An entry or check without `@` happens every day.

## Literals

| Literal | Days |
|---------|------|
| `*` | every day |
| `fri`, `friday` | every Friday; also `mon` … `sun` |
| `weekday`, `weekend` | Monday to Friday, or Saturday and Sunday |
| `15th` | the 15th of every month |
| `last` | the last day of every month |
| `2nd mon`, `last fri` | the second Monday, or the last Friday, of every month |
| `jan 1`, `jan 1st`, `feb last` | that day every year |
| `jul`, `july` | every day of July, every year |
| `q2` | every day of April, May and June, every year |
| `2026` | every day of 2026 |
| `jul 2026`, `q3 2027` | every day of that month or quarter, once |
| `2026-04-15` | that day |

A day past the end of a short month falls on its last day, so `30th` is
February 28 (or 29). Write years in full.

## Ranges

| Range | Days |
|-------|------|
| `2026-01-01..2026-04-01` | from the first date up to, but not including, the second |
| `2026-01-01..=2026-03-31` | from the first date through the second |
| `2026-07-01..`, `jul 2026..`, `2027..` | from that day, or the start of that month or year, on |
| `2026..2028`, `2026..=2028` | 2026 and 2027, or 2026 through 2028 |
| `sep..=jun` | September through June, every year |
| `q1..=q2` | January through June, every year |

`..` leaves out where it ends, and `..=` includes it. Both ends of a range need
a year, or neither; months wrap past December.

## Operators

| Operator | Days |
|----------|------|
| `a \| b` | the days in either |
| `a & b` | the days in both |
| `a - b` | the days in `a` but not `b` |
| `a / n` | every `n`th day of `a` |

`/` binds tightest, then `-`, then `&`, then `|`, so `15th | last & q2` is
`15th | (last & q2)`. Parentheses group as usual.

```
last - dec 31              // month ends, except the year's last
weekday - fri              // Monday to Thursday
(mon | wed) & 2026..       // Mondays and Wednesdays from 2026
```

### Every `n`th day

`/ n` keeps every `n`th day of a set, counting from the start of the range
it's intersected with:

```
2026-01-02.. & fri / 2     // every second Friday, from January 2nd
2026-01-01.. & * / 3       // every third day
2026-01-01.. & last / 3    // every third month end: Jan 31, Apr 30, Jul 31…
```

It counts the first matching day on or after the start as the first. Without a
range to count from, it's an error. The count carries on through days that
other parts of the set leave out, so
`2027-01-01.. & fri / 2 & sep..=jun` keeps its rhythm over the summer.

## Naming sets of days

`let` names a set of days, to use wherever a set of days goes:

```
let payday = 15th | last
let school = sep..=jun

entry paycheck @ payday { income.salary -> assets.cash : 4_000 }
entry daycare  @ 1st & school { assets.cash -> expenses.daycare : 2_000 }
```

A named set can leave out the range that `/ n` counts from, and get it where
it's used: `let biweekly = fri / 2` and then `@ 2026-01-02.. & biweekly`.

## Keeping some months

For an entry that moves a rate, like `24_000/year`, the parts of its set of
days play different roles (see [Entries](./entries.md#rates)):

- **Days of the week, month or year** (`fri`, `15th`, `jan 1`, `/ n`) set its
  rhythm: when it fires.
- **Months and quarters of every year** (`jul`, `q2`, `sep..=jun`) keep some
  months and leave the others out.
- **Anything with a year** (`2026`, `jul 2026..`, dates and ranges of them)
  says when it's active.

A rate is split between the firings in each period that fit the rhythm and the
months kept, including firings before the entry becomes active. So
`@ 1st & sep..=jun` splits `24_000/year` into ten firings of 2,400, and
`@ 2026-07-01.. & last` posts a twelfth of it each month from July, half of it
in 2026.

Periods the months leave out don't roll into the next firing: `@ 1st &
sep..=jun` moves `1_000/month` as 1,000 on September 1st, not 3,000. A period
is left out if the months kept include none of its days, or none of the days
the entry would have fired on in it.

## Common schedules

| Schedule | Set of days |
|----------|-------------|
| daily | `*`, or leave out `@` |
| weekly | `sun`, `fri` |
| semi-monthly | `15th \| last` |
| monthly | `last`, `1st` |
| quarterly | `mar 31 \| jun 30 \| sep 30 \| dec 31` |
| yearly | `dec 31`, `jan 1` |
| biweekly | `2026-01-02.. & fri / 2` |
| some dates | `2026-04-15 \| 2026-10-15` |
