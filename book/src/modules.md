# Modules

Modules organize a model: its salary in one, its loans in another, each with
names of its own. A module can hold any declaration: values, sets of days,
functions, entries, checks, balances and other modules.

## Syntax

```
mod <name>
mod <name> { <declarations> }
```

`mod loans { … }` declares a module in place. `mod loans`, without braces,
reads the module from the file `loans.saldo`.

## Example

`budget.saldo`, the file you run:

```
balances @ 2026-01-01 { assets.cash = 5_000 }

mod pay
mod loans

check assets.cash >= 0
```

`pay.saldo`:

```
let salary = 95_000/year

entry paycheck "Paycheck" @ 15th | last {
  income.salary -> assets.cash : salary
}
```

`loans.saldo`:

```
balances @ 2026-01-01 { liabilities.loans.car = 30_000 }

let rate = 5%/year

entry car "Car loan payment" @ last {
  assets.cash -> expenses.interest.car : liabilities.loans.car * rate
  assets.cash -> liabilities.loans.car : 600
}
```

```
saldo budget.saldo --from 2026-01-01 --to 2026-12-31
```

## Names

Everything a module declares is named by its path from the top of the model:
`pay.salary`, `loans.rate`, `loans.car`. Inside a module, a name is looked up
in the module first and then at the top of the model, so a module can use what
the main file declares, and its own names, without their paths. Anything else
is named in full:

```
let inflation = 3%

mod pay {
  let salary = 95_000/year
  let raised = salary * (1 + inflation)
}

mod tax {
  let rate = 25%
  let withheld = pay.salary * rate
}
```

Accounts aren't in modules: there's one set of accounts for the whole model,
and any module can move money into any of them. That keeps `expenses.insurance`
together even if insurance is paid in several modules.

Names that start accounts (`assets`, `liabilities`, `income`, `expenses`,
`equity`) or sets of days (like `may`, `fri` or `last`) can't name modules or
anything else.

## Functions that make modules

A function can make a module, to use the same entries more than once. It ends
with `mod { … }` instead of a value:

```
fn loan(debt, interest, rate, payment) {
  mod {
    entry repay @ last {
      let accrued = debt * rate
      debt        -> interest : accrued
      assets.cash -> debt     : min(payment, debt + accrued)
    }
  }
}

mod car = loan(
  debt:     liabilities.loans.car,
  interest: expenses.interest.car,
  rate:     6%/year,
  payment:  600,
)
mod furniture = loan(liabilities.loans.furniture, expenses.interest.furniture, 8.5%/year, 640)
```

`mod car = loan(…)` makes the module `car`, with the function's parameters
standing for the arguments: `car.repay` moves money out of
`liabilities.loans.car`, and `furniture.repay` out of
`liabilities.loans.furniture`. An argument can be an account, a value, a
number or a set of days, and means what it means where the call is. Inside
the module, names are looked up in the module and then at the top of the
model, as in any module; it can't declare a name that's also a parameter.

## Files

Modules in files follow the same layout as Rust's: `mod pay` in the file you
run reads `pay.saldo` from the same directory, and the modules in `pay`, in
its own file or written in it, read their files from the directory `pay`:

```
budget.saldo     mod loans
loans.saldo      mod car
loans/car.saldo
```

Each file is one module, read once. Where a `mod` is written barely matters:
entries that fire on the same day and don't read what each other moves fire in
the order they're written, with a module's declarations where its `mod` is.
