# How a simulation runs

```
saldo budget.saldo --from 2026-01-01 --to 2026-12-31
```

saldo simulates one day at a time and reports every day from `--from` to
`--to`, inclusive.

A model that draws at random (see [Random draws](./distributions.md)) draws
from a seed. `--seed N` sets it, and the same seed repeats a run exactly;
without one, saldo picks a seed at random and prints it. `--runs` simulates
many runs at once (see [Many runs](./monte-carlo.md)).

`--scenario NAME` simulates the model with one of its scenarios (see
[Scenarios](./scenarios.md)).

## Warm-up

If an account opens before `--from`, the simulation starts on the earliest
opening date instead. Entries fire normally during this warm-up, but their
transactions aren't printed. The opening balances you see are the balances on
`--from`, after the warm-up.

Accounts without an opening balance start at zero on the first simulated day.
Totals like `pay.contribution[year]` also start at zero on the first simulated
day. If that day falls partway through a period and an entry the total counts
would already have fired earlier in it, saldo warns that the total is missing
amounts. To fix it, simulate from the start of the period, open an account by
then so the warm-up covers it, or say what the total was in a `balances` block
(see [Accounts](./accounts.md#totals-so-far)).

## Each day

Every simulated day runs these steps in order:

1. Totals reset if the day starts a new month, quarter or year.
2. Values are evaluated. They depend on nothing but the date, so this happens
   once a day.
3. Accounts whose opening date is today get their opening balance.
4. Entries whose days include today fire. An entry fires after the entries
   that move what it reads, so it sees what they moved, and after the
   entries it names with `after`; other entries fire in the order they're
   written (see [Entries](./entries.md#which-fires-first)). Within one entry,
   every move is worked out from the balances before the entry fired, and
   its own tags' totals don't include the firing in progress.
5. Checks whose days include today are checked against the balances and
   values after all entries fired.

An evaluation error stops the simulation. A failing check doesn't: saldo
reports it once the output is written, and exits with status 3 (see
[Checks](./checks.md)). Checks during the warm-up run too, but don't count.

## Output

The ledger output (the default) starts with an `opening-balances` transaction
for the balances on `--from`, balanced against `Equity:OpeningBalances`. An
account that opens later gets its own `opening-balances` transaction on its
opening date. After that comes one transaction per entry firing, skipping
firings that move no money.

The CSV output (`--format csv`) has one row per day with every account's
balance at the end of that day, ordered by the accounts' names. Balances read
as their kind reads (see [Accounts](./accounts.md#kinds)), so what you owe on a
loan is positive.

`--every week`, `month`, `quarter` or `year` reports the last day of each
period instead of every day, and `--to` if it's partway through one.
`--series` picks the columns, named as given:

```
saldo budget.saldo --from 2026-01-01 --to 2030-12-31 --format csv \
  --every month --series assets.cash,assets,liabilities,prices
```

Each is an account; a group, which reports the sum of its accounts, so
`assets` and `liabilities` give net worth; or a value, like `prices`, which
is empty on days it has no value.
