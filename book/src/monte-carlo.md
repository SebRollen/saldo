# Many runs

A model that draws at random (see [Random draws](./distributions.md)) gives
one possible future per seed. `--runs` simulates many of them and writes them
all as one CSV, for a spreadsheet, a notebook or a database to summarize:

```
❯ saldo budget.saldo --from 2026-01-01 --to 2029-12-31 --runs 1000 > runs.csv
note: drew with seed 8812047; pass --seed 8812047 to reproduce
```

It's the CSV of a single run (see [How a simulation
runs](./simulation.md#output)) with each run's number and seed first, one row
per run and day, in order of run:

```
"run","seed","date","Assets:Cash","Expenses:Groceries","Income:Salary"
1,9306316622,2026-01-01,5000.00,0.00,0.00
1,9306316622,2026-01-02,5000.00,0.00,0.00
…
2,1137475080,2026-01-01,5000.00,0.00,0.00
```

`--every` and `--series` work as for a single run, and keep the file small:
a model with 40 accounts over 4 years is about 600 MB for 1,000 daily runs,
and 20 MB at month-ends:

```
saldo budget.saldo --from 2026-01-01 --to 2029-12-31 --runs 1000 \
  --every month --series assets.cash,assets,liabilities,prices
```

## Seeds

`--seed` sets the seed of all the runs, so the same seed repeats them all.
Each run draws with its own seed, made from that one and its number, and
that's the seed in the CSV. Passing a run's seed without `--runs` repeats
just that run, with its ledger:

```
saldo budget.saldo --from 2026-01-01 --to 2029-12-31 --seed 1137475080
```

## Checks

A check that fails doesn't stop a run. Once every run is written, saldo
reports each check that failed, in how many runs, and the run in which it
failed first, to replay:

```
Error: check failed in 412 of 1000 runs, first on 2027-07-01 in run 17 (seed 4471093281): -1371.42 >= 0
```

and exits with status 3, as a single run does. An error, like a negative
standard deviation, stops all the runs and names the seed it happened with.

## Speed

The runs are spread over every core. Each run is as fast as a single run;
runs don't keep their transactions, since the CSV doesn't need them.
