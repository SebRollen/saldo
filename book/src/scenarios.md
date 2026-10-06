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
without braces, reads it from the file `prepay.saldo`, next to the model.
Scenarios are declared at the top of the file saldo runs, not in modules.

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
the model as it's written.

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
