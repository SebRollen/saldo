# Monte Carlo

Status: designed, not started.

A model that draws at random (see `plans/random-distributions.md`) gives one
possible future per seed. Monte Carlo runs it many times and writes every
run's balances as one CSV, for other tools to summarize. Any single run can
still be replayed as an ordinary ledger with its own seed.

## What users run

```
saldo model.saldo --from 2026-10-01 --to 2029-12-31 --runs 1000 > runs.csv
saldo model.saldo --from 2026-10-01 --to 2029-12-31 --runs 1000 --every month --series assets.cash,prices
saldo model.saldo --from 2026-10-01 --to 2029-12-31 --seed 4471093281   # replay one run
```

## Decisions

### Checks don't stop a run

A failing check no longer stops the simulation, in a single run or in Monte
Carlo. The run goes on to `--to`, and saldo records, for each check, the first
day it failed and how many days it failed on.

- **A single run** writes all its output to stdout as usual, then reports
  each failed check to stderr and exits with status 3: failing checks are a
  result, not a broken model. One diagnostic per check, not per day, pointing
  at the check:

  ```
  Error: check failed on 2027-07-01: -1371.42 >= 0, and on 152 more days, last on 2027-12-10
  ```

  Exit status 1 stays for errors (the model doesn't parse, resolve or
  evaluate), and 2 for bad arguments, so scripts can tell them apart.
- **Under `--runs`**, it's the same, one diagnostic per check across all the
  runs, naming the run where it first failed so it can be replayed:

  ```
  Error: check failed in 412 of 1000 runs, first on 2027-07-01 in run 17 (seed 4471093281)
  ```

  and the exit status is 3 if a check failed in any run. Errors still stop
  everything, naming the run's seed.
- An error evaluating a check, like dividing by zero, is still an error.
- Only days from `--from` count. Checks during the warm-up still run, since
  nothing is reported for those days, but failures there are ignored.
- `Output` gets `failed_checks: Vec<CheckFailure>`, with the check's span, the
  first failing day and its message, the number of days and the last day.
  `run` and `run_file` return `Ok` when only checks fail; the CLI decides the
  exit status.

### Seeds

`--seed` is the master seed. Run *i* (1 to N) draws with
`random::mix(master, i)`, which is what the output calls its seed, so
`--seed <that>` without `--runs` replays it as a ledger. Without `--seed`, the
master seed is random and printed to stderr, as for a single run.

### Output

Stdout holds the output and nothing else; stderr holds warnings, notes and
failed checks.

Under `--runs`, the output is CSV, and `--format ledger` is an error. It's the
CSV of a single run with `run` and `seed` columns first, one row per run and
day, in run order:

```
"run","seed","date","Assets:Cash","Expenses:Groceries"
1,9306316622,2026-10-01,16743.24,0.00
1,9306316622,2026-10-02,16743.24,185.37
…
2,1137475080,2026-10-01,16743.24,0.00
```

Tools that want one row per run, day and account can melt it.

### Sampling: `--every`

Daily is the default, as now. `--every week|month|quarter|year` reports the
last day of each period instead, and `--to` if it's partway through one. It
applies to CSV, in a single run or many; `ledger` lists every transaction, so
`--every` with it is an error.

For scale: a model with 44 accounts over 3 years is about 460 MB of CSV for
1,000 daily runs, 15 MB at month-ends.

### Series: `--series`

`--series assets.cash,assets,prices` picks the CSV's columns. Each is an
account, a group (the sum of its accounts, so `assets` minus `liabilities` is
net worth), or a value, and its column is named as given. Without it, every
account is reported, named as the ledger names it, as now.

Values are recorded from the day's evaluation (`State::params`), so reporting
one costs nothing extra.

### Running

Runs are independent: compile once, then simulate on every core with
`std::thread::scope` and `std::thread::available_parallelism`, so no new
dependencies. Output is in run order whatever order runs finish in: each run
is written as soon as the runs before it are, so memory holds only the runs
waiting their turn.

## Implementation sketch

1. **Checks that don't stop runs** (useful on its own): `run_checks` records
   failures in `State` instead of returning them; `SimLog` and `Output` carry
   them; `main.rs` prints them to stderr after the output and exits with 3.
   Update `checks.md`, `simulation.md`, the README and the tests that expect a
   failing check to be an error.
2. **`--every` and `--series`** for single runs: sample `SimLog::snapshots`;
   record values in snapshots; resolve `--series` names against the model (a
   new `Model` lookup for accounts, groups and values).
3. **`--runs`**: `RunOpts::runs`, per-run seeds, a `run_many` that hands back
   per-run results in order, the `run` and `seed` columns, failed checks
   across runs, and parallel workers.
4. **Docs**: a book chapter on Monte Carlo, the CLI options in the README and
   `simulation.md`.

## Deferred

- Summaries in saldo (percentiles, how often each check fails), and a format
  with one row per run, if computing them elsewhere gets tedious.
- Check outcomes as CSV columns, so other tools can count failures of
  conditions that aren't simply balances.
- Parquet output, if CSV gets too big even sampled.
