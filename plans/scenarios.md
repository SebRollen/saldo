# Scenarios

Status: planned.

A model describes one future, with one set of decisions. A scenario is another
set: pay the car off early, change childcare, buy a house. It's written as
changes to the model, so it can be run on its own, or alongside the model as it
is and other scenarios. Every scenario draws the same random numbers, so the
difference between two of them comes from the decision and not from noise.

## What users write

For a model with modules `pay`, `loans` and `childcare`:

```
scenario prepay {
  // Replaces `loans.car_payment` wherever it's read.
  let loans.car_payment = 1_200

  // `pay.salary` on the right is the salary this replaces.
  let pay.salary {
    2028.. => pay.salary * 80%
    _      => pay.salary
  }
}

scenario daycare {
  drop childcare.nanny @ 2027-09-01..

  entry childcare.daycare "Daycare" @ 1st & 2027-09-01.. & sep..=jun {
    assets.cash -> expenses.childcare.daycare : 2_100 * prices
  }
}

scenario house     // reads house.saldo
```

## What users run

```
saldo model.saldo --from 2026-10-01 --to 2030-12-31 --scenario prepay
saldo model.saldo --from 2026-10-01 --to 2030-12-31 --runs 1000 --every month \
  --series assets,liabilities --scenario base,prepay,daycare,prepay+daycare > runs.csv
```

## Decisions

### Declaring scenarios

- `scenario <name> { … }` declares a scenario in place, and `scenario <name>`
  reads it from `<name>.saldo`, next to the model, as `mod` does. Modules in a
  scenario read their files from `<name>/`.
- Scenarios are declared only at the top of the file saldo runs: not in
  modules, and not in other scenarios.
- A scenario's name is declared at the top of the model with everything else,
  so it can't also name a module or a value. `base` is the model with no
  scenario, so it can't name one.
- `scenario` and `drop` become keywords.

### Inside a scenario

A scenario holds `let`, `fn`, `entry`, `mod`, `check` and `balances`
declarations, and `drop`.

**Names are paths.** `let loans.car_payment = …` is declared in the module
`loans`, as if it were written there: its names are looked up in `loans` and
then at the top of the model. A name without dots is declared at the top. The
modules on the path must exist, so `let loan.car_payment` is an error, with a
suggestion.

**Replace or add.** If the model already declares the path, the scenario's
declaration replaces it, everywhere it's read. Otherwise the scenario adds it.
A replacement is the same kind as what it replaces: a `let` for a `let`, an
`fn` for an `fn`, an `entry` for an `entry` and a `mod` for a `mod`.

- `mod x { … }`, `mod x` and `mod x = f(…)` replace the whole module. Anything
  that reads a name the new module doesn't have is an error, as usual.
- Scenarios apply before `mod x = f(…)` is expanded, so replacing a function
  that makes modules changes every module it makes.
- A module made by a function can't be changed piece by piece:
  `let car.rate = …`, where `mod car = loan(…)`, is an error that says to
  replace `mod car = loan(…)` instead.

**Reading what it replaces.** In a `let` or `fn` that replaces another, its
own name means the one it replaces, as in Rust's `let x = x * 2`. Everywhere
else, the rest of the scenario included, the name means the replacement.
Otherwise, a function still can't call itself.

**Typos.** A value or a function that a scenario adds and nothing reads is an
error, since it's most likely a misspelled replacement:

```
Error: nothing reads `loans.car_paymnet`; did you mean `loans.car_payment`?
```

Entries and modules a scenario adds do something on their own, so they aren't
checked for this.

**Checks** in a scenario are added to the model's. The model's checks have no
names, so they can't be replaced, but `drop` can stop them (below).

**Balances** in a scenario replace the opening balance of each account they
name, its date included, and the totals so far they set. Other accounts keep
theirs.

**`drop <path> [@ <days>]`** stops an entry firing on those days, or every day
without `@`. For a module, it stops every entry in it firing, and every check
in it being checked, including those in modules inside it. It changes days and
nothing else: `drop x @ d` makes `x`'s days its days `- d`. A dropped entry is
still declared, so `after` and tag totals that name it still work, and its
totals stay where they were. The days are read at the top of the model,
wherever the entry is. Values and functions can't be dropped; replace them
instead.

### Combining scenarios

`a+b`, on the command line only, applies `a`'s changes and then `b`'s.

- It's an error if both declare the same path, or one changes a module and the
  other changes something in it (`mod childcare` in one, `let childcare.rate`
  in the other).
- Drops apply after replacements, so `b` can drop an entry `a` replaced. Days
  either drops are dropped.
- Checks from both are added.

### Running

`--scenario NAME,...` lists what to run. Each is `base`, a scenario, or
scenarios joined by `+`. A name that isn't a scenario is an error, with a
suggestion, as `--series` does.

- **One scenario** runs the model with it, and writes what the model alone
  would: a ledger or CSV, and `--runs` as now.
- **Several** write CSV with a `scenario` column, and `--format ledger` is an
  error, as under `--runs`:
  - a single run has `scenario`, `date`, …, with rows by scenario, then date;
  - under `--runs` it's `run`, `seed`, `scenario`, `date`, …, with rows by run,
    then scenario in the order given, then date.
- **Same draws.** Run *i* draws with the same seed, `random::mix(master, i)`,
  in every scenario, so `--seed <that> --scenario daycare` replays that run of
  `daycare` as a ledger.
- **Columns.** Without `--series`, every account in any of the scenarios; an
  account a scenario doesn't have reads 0 in it. A name in `--series` must
  name something in at least one scenario. Accounts and groups read 0 where
  they don't exist, and values are left empty.
- **Checks** are reported once for each scenario they fail in, naming it:

  ```
  Error: check failed in daycare on 2027-07-01: -1371.42 >= 0, and on 152 more days, last on 2027-12-10
  Error: check failed in 412 of 1000 runs of daycare, first on 2027-07-01 in run 17 (seed 4471093281)
  ```

  The exit status is 3 if a check failed in any of them.
- **Errors.** Every scenario is parsed, but only those asked for are resolved
  and checked, each as a model of its own. Errors found with a scenario carry
  a note naming it, since one can show up in the model's code, like a
  replacement with the wrong unit.
- **In parallel**, a worker simulates one run of one scenario at a time, and
  they're handed over in the order above.

### Draws

A draw's key comes from its declaration's full name and its place in the
declaration (`number_draws`), so:

- every declaration a scenario doesn't touch draws the same numbers in every
  scenario;
- a replacement draws what it replaces drew: `normal(0.4%, 4%)` in place of
  `normal(0.5%, 4%)` makes each draw 0.1% smaller;
- when a replacement reads what it replaces, the replaced one keeps those
  draws, and the replacement's own draws are keyed by its name and the
  scenario's, so the two don't draw the same numbers;
- entries a scenario adds draw numbers of their own, and the model's entries
  draw the same with or without them.

## Implementation sketch

1. **Syntax**: `scenario` and `drop` tokens; `Decl::Scenario { name, body }`,
   whose body is a file to read or a list of changes, each a declaration with
   the module path before its name, or a drop with its path and days. The
   parser allows `scenario` only at the top of a file and the parts of a
   change only in a scenario. Update the tree-sitter grammar and highlights.
2. **Loading**: `sources` reads `scenario x` from `x.saldo`, and the files of
   modules in it from `x/`, as for modules. `Program` gets the scenarios apart
   from its declarations.
3. **Applying** (`src/scenario.rs`): `apply(program, scenarios)` finds each
   change's module, checks its kind, and replaces or adds it. A replaced `let`
   or `fn` is kept under a name users can't write (with `$`, as loops do), and
   `Decl::Let` and `Decl::Fn` get `replaces: Option<String>` to name it. It
   also reports conflicts between combined scenarios, and hands the resolver
   the drops and the names that scenarios added.
4. **Resolver**: in a replacement, a lookup that finds its own name finds the
   one it replaces. `number_draws` takes a key apart from the full name. Drops
   subtract their days, lowered at the top of the model, from the schedules of
   the entries and checks they name. Values and functions that scenarios added
   and nothing reads are errors.
5. **Library**: `load_file` keeps the program, and `Model::scenario("a+b")`
   applies, resolves and compiles that combination; `Error::UnknownScenario`
   for names that aren't scenarios; errors get the note naming the scenario.
   Columns are shared across scenarios, and `simulate_runs` takes a list of
   models and hands over each run of each one.
6. **CLI**: `--scenario`, the `scenario` column, checks per scenario, and the
   usage and help text.
7. **Docs**: a book chapter on scenarios after "Many runs", the syntax and
   `--scenario` in the README and `simulation.md`, and same-seed comparisons
   in `monte-carlo.md`.

## Deferred

- `--set name=value`, for a one-off change without writing a scenario.
- Scenarios that take arguments.
- Combining scenarios in the model, like `scenario both = a + b`.
- Changing a module made by a function piece by piece.
- Naming checks, so scenarios can replace them.
- A warning when a scenario adds an entry whose name is close to one the model
  has, which is most likely a misspelled replacement.
- Summaries comparing scenarios, like the difference between them in each run,
  beyond the CSV that lines runs up.
