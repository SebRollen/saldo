# Imports

A model can be split across files: salary in one, loans in another, and a
main file that brings them together. `import` reads another file into the
model.

## Syntax

```
import "<path>"
```

The path is relative to the directory of the file the import is in, or
absolute. Imports go between declarations, anywhere in a file.

## Example

`budget.saldo`, the file you run:

```
account Assets:Cash = 5_000 @ 2026-01-01

import "salary.saldo"
import "loans/car.saldo"

assert that Assets:Cash >= 0
```

`salary.saldo`:

```
account Income:Salary

param salary = 95_000 per year

entry monthly on the 15th and last day "Paycheck" {
  Assets:Cash = salary
  Income:Salary
}
```

`loans/car.saldo`:

```
account Liabilities:Loans:Car = -30_000 @ 2026-01-01
account Expenses:Interest:Car

param car_loan_rate = 5% per year

entry monthly on the last day "Car loan payment" {
  Expenses:Interest:Car = -Liabilities:Loans:Car * car_loan_rate
  Liabilities:Loans:Car = 600
  Assets:Cash
}
```

```
saldo budget.saldo --from 2026-01-01 --to 2026-12-31
```

## One model, many files

All the files make up one model, with one set of names. A file can use any
account, param, schedule, function or entry alias declared in any other file,
whether it imports that file or not: the car loan pays from `Assets:Cash`,
which `budget.saldo` declares. Names still have to be unique across files, so
declaring `Assets:Cash` in two files is an error.

Each file is read once, no matter how many files import it. Two files can
import a shared file of accounts, and files can import each other.

## Order

An import works as if the file's declarations were written where it's first
imported. That order matters in two places: entries that fire on the same day
fire in declaration order, and CSV columns follow the order accounts are
declared in. Above, a paycheck on the last day of the month comes before that
day's car loan payment, because `salary.saldo` is imported first.
