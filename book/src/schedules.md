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
| `weekly` | Every Monday |
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
defaults to Monday:

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

## Literal date lists

A comma- or `and`-separated list of ISO dates fires on exactly those days:

```
2026-04-15
2026-04-15 and 2026-10-15
2026-01-01, 2026-07-04, 2026-12-25
```

## Default behaviors summary

| Form | Default firing day |
|------|--------------------|
| `weekly` | Monday |
| `every week` | Monday |
| `monthly` | Last day of month |
| `every month` | Last day of month |
| `quarterly` | Quarter-end (Mar 31 / Jun 30 / Sep 30 / Dec 31) |
| `yearly` / `every year` | Dec 31 |
| `every <month-name>` | Last day of that month |
