# Schedules

A schedule determines *when* an entry fires or an assertion is checked.
Schedules appear inline on entries and assertions, or they can be named
and reused.

## Named schedules

```
schedule <name> = <schedule-expression>
```

```
schedule semi_monthly   = every month on the 15th, last day
schedule every_two_weeks = every second friday from 2026-01-02
```

A named schedule is referenced by its identifier wherever a schedule is
expected.

## Adverbial shortcuts

The simplest schedules are single-word adverbs. Each has a sensible default
when no further detail is given.

| Keyword | Fires on |
|---------|----------|
| `daily` | Every day |
| `weekly` | Every Sunday |
| `monthly` | Last day of every month |
| `quarterly` | Mar 31, Jun 30, Sep 30, Dec 31 |
| `yearly` / `annually` | Dec 31 |

Each adverb accepts an optional `on` clause to override the default:

```
weekly on friday                         # every Friday
weekly on monday and wednesday           # Mon and Wed each week
monthly on the 1st                       # first of every month
monthly on the 15th, last day            # 15th and last day
yearly on jan 1st                        # New Year's Day
yearly on may first, jul last            # May 1 and July 31 each year
```

## The `every` form

The `every` keyword gives you full control.

```
every <period> [from <date>]
every <n> <period> from <date>
```

### Periods

**`day`** — fires every day (or every *n* days from a start date):

```
every day
every 3 days from 2026-01-01
```

**`week [on <days>]`** — fires every week on the given day(s). Without `on`,
defaults to Sunday:

```
every week
every week on thursday
every week on weekend and friday
```

**`<weekday>`** — shorthand for `every week on <weekday>`:

```
every friday
every monday
every weekday
```

**`month [on the <occurrences>]`** — fires every month. Without `on the`,
defaults to the last day of the month:

```
every month
every month on the 1st
every month on the last day
every month on the 2nd monday
every month on the 3rd thursday, 15th
```

**`<month-name> [<ordinal>]`** — fires once a year in the named month. Without
an ordinal, defaults to the last day of that month:

```
every january
every january 1st
every december last
every aug 15th
```

**`quarter`** — fires at the end of each quarter:

```
every quarter
```

**`year [on <month> <ordinal>, ...]`** — fires once a year. Without `on`,
defaults to Dec 31:

```
every year
every year on april 15th
every year on jan 31, jul 31
```

### Skipping occurrences

Prefix the period with a count to fire every *n*th occurrence. A `from` date
is required to anchor the sequence:

```
every 2 weeks from 2026-01-05          # biweekly starting Jan 5
every second friday from 2026-01-02    # alternate Fridays
every 3 months from 2026-01-01         # quarterly with custom anchor
every 2 years from 2026-01-01          # biennially
```

Ordinal words (`second`, `third`, `fourth`, …, `tenth`) and numeric suffixes
(`2nd`, `3rd`, `4th`, …) are both accepted.

`every <n> weeks on <days>` counts calendar weeks (Monday to Sunday) from the
week containing the `from` date, so all the listed days of a week fire
together. `every <n> <weekday>` counts occurrences of that day instead,
starting with the first one on or after the `from` date.

## Keeping some months

`in` after a schedule keeps only the days in a list of months, quarters and
years, the same list a param's arms take (see
[Params](./params.md#conditions)). It goes after the days the schedule fires
on, and before `from`:

```
monthly on the 1st in sep through jun          # the school year
every friday in q2, q4                         # Fridays in two quarters
yearly on jan 1st in 2027 through 2029         # three New Years
every second friday in sep through jun from 2026-01-02
```

A counted schedule, like `every second friday`, keeps counting through the
days `in` leaves out, so it picks up its rhythm where it would have been.

The periods `in` leaves out don't roll into the next firing (see
[Rates](./entries.md#rates)). An entry `monthly on the 1st in sep through jun`
posts `1_000 per month` as 1,000 on September 1st, not 3,000, and splits
`24_000 per year` over its ten firings, 2,400 each. A period is left out if
`in` keeps none of its days, or none of the days the schedule would have
fired on in it.

## Literal date lists

A comma- or `and`-separated list of ISO dates fires on exactly those days:

```
2026-04-15
2026-04-15 and 2026-10-15
2026-01-01, 2026-07-04, 2026-12-25
```

## Default behaviors summary

Without an `on` clause, every period fires on its last day. Weeks run
Monday to Sunday.

| Form | Default firing day |
|------|--------------------|
| `weekly` | Sunday |
| `every week` | Sunday |
| `monthly` | Last day of month |
| `every month` | Last day of month |
| `quarterly` | Quarter-end (Mar 31 / Jun 30 / Sep 30 / Dec 31) |
| `yearly` / `every year` | Dec 31 |
| `every <month-name>` | Last day of that month |
