# Scenarios

A model describes one future, with the decisions you've made so far. A
scenario describes another: pay the car off early, swap the nanny for daycare,
buy a house. It's written as changes to the model, and you run the model with
it.

## Syntax

```
scenario <name> { <changes> }
scenario <name>
```

`scenario prepay { … }` declares a scenario in place. `scenario prepay`,
without braces, reads it from the file `prepay.saldo`, next to the file it's
declared in. Scenarios can be declared in modules too, and a scenario can be
declared in several places, which are one scenario (see
[below](#in-modules)).

## Example

`budget.saldo`:

```
balances @ 2026-01-01 { assets.cash = 10_000 }

mod pay {
  let salary = 90_000/year

  entry paycheck "Paycheck" @ 15th | last {
    income.salary -> assets.cash : salary
  }
}

mod loans {
  let car_payment = 600

  balances @ 2026-01-01 { liabilities.car = 18_000 }

  entry car "Car payment" @ 1st {
    assets.cash -> liabilities.car : min(car_payment, liabilities.car)
  }
}

entry nanny "Nanny" @ fri {
  assets.cash -> expenses.childcare : 39_000/year
}

scenario prepay {
  let loans.car_payment = 1_500
}

scenario daycare {
  drop nanny @ 2026-09-01..

  entry daycare "Daycare" @ 1st & 2026-09-01.. {
    assets.cash -> expenses.childcare : 2_100
  }
}
```

```
saldo budget.saldo --from 2026-01-01 --to 2026-12-31 --scenario prepay
```

`--scenario` simulates the model with the scenario, and writes what it
would without one: a ledger, or CSV with `--format csv`. `--scenario base` is
the model as it's written. To compare scenarios, run them side by side (see
[below](#side-by-side)).

## Replacing and adding

A declaration in a scenario is named by its path, like `loans.car_payment`,
and is declared in that module, as if it were written there: its names are
looked up in the module and then at the top of the model. A name without dots
is declared at the top.

If the model already declares the path, the scenario's declaration replaces
it, everywhere it's read. Otherwise the scenario adds it. A replacement is
declared with the same keyword as what it replaces: `let` for a value or a set
of days, `fn` for a function, `entry` for an entry and `mod` for a module.

```
scenario raise {
  let pay.salary = 100_000/year           // replaces pay.salary

  entry pay.bonus @ dec 15 {              // adds pay.bonus
    income.bonus -> assets.cash : 10_000
  }
}
```

A `mod` replaces the whole module: `mod loans { … }`, `mod loans`, which reads
a file, and `mod loans = refinance(…)`. Replacing a function that makes
modules changes every module it makes. A module that a function makes can only
be replaced whole: to change the car loan made by `mod car = loan(…)`, replace
that line.

## In modules

A scenario declared in a module changes that module, as if its changes were
written there: `let rent = 2_000` in a scenario in `housing` replaces
`housing.rent`, `drop storage` drops `housing.storage`, and the names its
declarations and checks read are looked up in `housing`, then at the top of
the model.

Blocks with one name, wherever they are, are one scenario. A move to New
Jersey changes rent and taxes, and each part can sit with what it changes:

`housing.saldo`:

```
let rent = 1_800

entry due "Rent" @ 1st {
  assets.cash -> expenses.rent : rent
}

scenario nj {
  let rent = 2_400
}
```

`tax.saldo`:

```
let state_rate = 6%

scenario nj {
  let state_rate = 5.5%
}
```

`--scenario nj` runs the model with both. As when [combining
scenarios](#combining-scenarios), it's an error if two of a scenario's blocks
change the same declaration, or one changes a module and the other something
in it.

`scenario nj` without braces, in `housing.saldo`, reads `housing/nj.saldo`,
and the modules in it are in `housing/nj/`. A module that a function makes
can't declare scenarios, and neither can a scenario.

## Reading what's replaced

In a value or a function that replaces another, its own name stands for the
one it replaces, as `let x = x * 2` does in Rust. Everywhere else, the rest of
the scenario included, the name means the replacement:

```
scenario part_time {
  let pay.salary {
    2027-07-01.. => pay.salary * 60%
    _            => pay.salary
  }
}
```

## Dropping

`drop` stops an entry firing, on some days or on all of them:

```
scenario daycare {
  drop nanny @ 2026-09-01..
}
```

The firings that are left move what they would have: each of the nanny's
Fridays until September still moves a 52nd of `39_000/year`. Dropping a module
drops every entry in it, and stops its checks being checked, including those
in the modules inside it.

A dropped entry is still declared, so `after` and its tags' totals still work,
and its totals stay where they were. Values and functions can't be dropped;
replace them instead.

## Checks and balances

Checks in a scenario are added to the model's, so a scenario can have goals
of its own:

```
scenario house {
  check assets.cash >= 20_000 @ 2027-06-01
}
```

A scenario's `balances` replace the opening balance of each account they name,
its date included, and the totals so far they set. Other accounts keep theirs.

## Combining scenarios

`+` runs the model with several scenarios, one after another:

```
saldo budget.saldo --from 2026-01-01 --to 2026-12-31 --scenario prepay+daycare
```

It's an error if two of them change the same declaration, or one changes a
module and the other something in it. Days that either drops are dropped, and
checks from both are checked.

## Side by side

Several scenarios, separated by commas, run side by side, as one CSV with a
`scenario` column:

```
saldo budget.saldo --from 2026-01-01 --to 2026-12-31 --every quarter \
  --series assets.cash,liabilities --scenario base,prepay,daycare
```

```
"scenario","date","assets.cash","liabilities"
base,2026-03-31,20950.00,16200.00
base,2026-06-30,31900.00,14400.00
…
prepay,2026-03-31,18250.00,13500.00
…
daycare,2026-12-31,58150.00,10800.00
```

The columns are the same for each: every account in any of them, or the
series asked for. An account a scenario doesn't have reads 0 there, and a value
it doesn't have is empty. A check that fails is reported for each scenario it
fails in, naming it. With `check assets.cash >= 30_000 @ 2026-06-30`:

```
Error: check failed in prepay on 2026-06-30: 26500 >= 30000
```

With `--runs` (see [Many runs](./monte-carlo.md)), each run comes first, then
each scenario in it, and run 17 draws with the same seed in every scenario. If
the groceries were drawn at random, say with `normal(250, 60)` each Sunday,
both would buy the same groceries in run 17, so the difference between `base`
and `prepay` there is what paying the car off early does, and not luck:

```
saldo budget.saldo --from 2026-01-01 --to 2026-12-31 --runs 1000 --every year \
  --series assets.cash,liabilities --scenario base,prepay
```

```
"run","seed","scenario","date","assets.cash","liabilities"
1,9716232063330790915,base,2026-12-31,40630.99,10800.00
1,9716232063330790915,prepay,2026-12-31,29830.99,0.00
2,13066309581097501094,base,2026-12-31,40924.92,10800.00
…
```

`--seed` with a run's seed and `--scenario prepay` repeats that run of
`prepay`, with its ledger.

## Mistakes

A value or a function that a scenario adds is an error if nothing reads it,
since it's most likely meant to replace one whose name is misspelled:

```
Error: nothing reads `loans.car_paymnet`
    ╭─[ budget.saldo:26:13 ]
    │
 26 │   let loans.car_paymnet = 1_500
    │             ─────┬─────
    │                  ╰─────── nothing reads `loans.car_paymnet`
    │                  │
    │                  ╰─────── did you mean to replace `loans.car_payment`?
────╯
```

An error that a scenario causes elsewhere in the model, like a replacement
with the wrong unit, names the scenario.

## Random draws

A draw depends on the seed, where it's written and when (see [Random
draws](./distributions.md)), so with the same `--seed`, a scenario draws what
the model does for everything it doesn't change. A replacement draws what it
replaces drew: replacing `normal(0.5%, 4%)` with `normal(0.4%, 4%)` makes
every draw 0.1% smaller. When a replacement reads what it replaces, the
replaced one keeps its draws, and the replacement's own draws are new.

That way, the difference between the model and a scenario run with the same
seed comes from what the scenario changes, not from drawing differently.

## Files

`scenario house` reads `house.saldo`, which holds what the braces would. The
files of modules a scenario declares are in the directory `house`, laid out as
the model's are:

```
budget.saldo         mod childcare, scenario house
childcare.saldo
house.saldo          mod mortgage, mod childcare.new_school
house/mortgage.saldo
house/childcare/new_school.saldo
```
