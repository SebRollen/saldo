# saldo

A small domain-specific language for personal financial simulation. You
describe accounts, values, and recurring cash flows in a plain-text
file, then ask saldo to simulate them over a date range and emit
transactions. The default output is plain-text accounting transactions
that can be imported into tools like ledger/hledger for reporting and
analysis.

## Example

```
balances @ 2026-01-01 {
  assets.cash      = 5_000
  liabilities.loan = 30_000
}

let interest_rate = 5%/year
let salary {
  2025-01-01..2025-04-16 => 80_000/year
  2025-04-16..           => 95_000/year   // promoted!
}

let payday = 15th | last

entry paycheck "Paycheck" @ payday {
  income.salary -> assets.cash : salary
}

entry accrual "Interest accrual" {
  liabilities.accrued_interest -> expenses.interest : liabilities.loan * interest_rate
}

entry loan_payment "Loan payment" @ 17th after accrual {
  assets.cash -> liabilities.accrued_interest : liabilities.accrued_interest
  assets.cash -> liabilities.loan             : 2_000
}

check assets.cash >= 0
```

When run through the `saldo` CLI, this file generates transactions:
```
❯ saldo budget.saldo --from 2026-01-01 --to 2027-01-01
2026-01-01 opening-balances
  Assets:Cash               5000
  Liabilities:Loan        -30000
  Equity:OpeningBalances   25000

2026-01-01 Interest accrual
  Expenses:Interest             4.11
  Liabilities:AccruedInterest  -4.11

2026-01-02 Interest accrual
  Expenses:Interest             4.11
  Liabilities:AccruedInterest  -4.11
…
2026-01-15 Paycheck
  Assets:Cash     3958.33
  Income:Salary  -3958.33
…
2026-01-17 Loan payment
  Liabilities:AccruedInterest     69.86
  Assets:Cash                  -2069.86
  Liabilities:Loan                 2000

2026-01-18 Interest accrual
  Expenses:Interest             3.83
  Liabilities:AccruedInterest  -3.83
…
```

The transactions can be piped into other PTA tools for reporting:
```
> saldo budget.saldo --from 2026-01-01 --to 2027-01-01 | hledger -f - bal
            75108.23  Assets:Cash
            25000.00  Equity:OpeningBalances
              904.10  Expenses:Interest
           -95000.00  Income:Salary
              -12.33  Liabilities:AccruedInterest
            -6000.00  Liabilities:Loan
--------------------
                   0
```


## Usage

```
saldo <path> --from YYYY-MM-DD --to YYYY-MM-DD [--format ledger|csv]
      [--every UNIT] [--series NAME,...] [--seed N] [--runs N] [--scenario NAME,...]
```

- `--from` / `--to` — inclusive date range to simulate
- `--format ledger` (default) — outputs double-entry ledger transactions
- `--format csv` — outputs the daily balance of every account as CSV
- `--every week|month|quarter|year` — with CSV, report the last day of each
  period instead of every day
- `--series assets.cash,assets,prices` — with CSV, report these accounts,
  groups of accounts (summed) or values instead of every account
- `--seed N` — the seed random draws come from; the same seed repeats a run
  (default: a random seed, which saldo prints)
- `--runs N` — simulate N runs, each with its own seed, as one CSV with each
  run's number and seed; `--seed` with a run's seed repeats it
- `--scenario NAME,...` — simulate the model with scenario NAME, or with the
  scenarios NAME joins with `+`, like `prepay+daycare`; several, like
  `base,prepay`, run side by side as one CSV with each one's name, drawing the
  same numbers in each

saldo exits with 0 when all is well, 1 for an error in the model, 2 for bad
arguments, and 3 when a check failed (it still writes all the output).

## Documentation

The full language reference is available at https://sebrollen.github.io/saldo/

## Language summary

### Accounts

```
balances @ 2026-01-01 {
  assets.cash      = 5_000
  liabilities.loan = 30_000
}
```

Accounts hold balances. Their names are paths of words joined by dots,
starting with their kind: `assets`, `liabilities`, `income`, `expenses` or
`equity`. Accounts don't need declaring: an account exists because an entry
moves money into or out of it, or because `balances` gives it an opening
balance. A path with accounts under it, like `assets.retirement`, is a group,
and reading it gives their sum.

A `balances` block can also say what an account or a tag has moved so far this
year, quarter or month, like `pay.paycheck.contribution[year] = 18_375`, for a
model that starts partway through a year.

Every account reads positive the way you'd say it: what you own, what you owe,
what you've earned or spent. Ledger output keeps ledger's signs, and each part
of a name is capitalized: `liabilities.accrued_interest` is
`Liabilities:AccruedInterest`.

### Values

```
let interest_rate = 5%/year
let salary {
  2026        => 95_000/year
  2027..      => 100_000/year compounding 3% every year
}
```

Values are named numbers, or bools, like whether we've moved yet. `24_500[year]` (or `[month]`, and so on) makes a
limit: an amount each period, which entries fill over the firings left in it,
so a job that starts in July still contributes the whole year's limit. A value
can be a constant, or take different values on different days, each arm covering a set of days and the first that covers a
day giving its value, like `jul | aug => 0` for a summer break, and grow, like
`2026.. => 1 compounding 3% every year`. `/year` (or `/day`, `/week`, `/month`,
`/quarter`) after a value makes it a rate: entries spread it over their
firings, so each year's paychecks add up to exactly the yearly salary. Values
depend on nothing but the date, so they can't read balances or totals; an
entry's `let` can.

### Sets of days

```
let payday = 15th | last
let school = sep..=jun
```

Sets of days say when entries fire and checks are checked. They're written
with literals like `fri`, `15th`, `last`, `2nd mon`, `jan 1`, `jul 2026`, `q2`
or a date, ranges like `2026-07-01..` or `sep..=jun`, and operators: `|` for
either, `&` for both, `-` for except, and `/ n` for every `n`th, counted from
the start of a range, like `2026-01-02.. & fri / 2`.

### Entries

```
entry paycheck "Paycheck" @ 15th | last {
  income.salary -> assets.cash       : salary
  assets.cash   -> assets.retirement : min(salary * 16%, max_401k - contribution[year]) #contribution
}
```

An entry fires on a set of days, or every day without `@`, and each line moves
an amount from one account to another. Every move balances, so every
transaction does, and the transaction shows what each account gets in the end.
The label is optional; without one, the transaction is named after the entry.

`if cond { … } else { … }` picks which moves an entry makes each time it
fires, and `for a in assets.retirement { … }` makes the same moves for each
account under a group. On a day several entries fire, an entry fires after the entries that
move what it reads. When two entries each read what the other moves, like the interest
accrual and the loan payment above, `after` says which goes first.

`#contribution` tags a move, and `contribution[year]` is what the tagged moves
moved so far this year (also `[quarter]` and `[month]`). Elsewhere, it's
`paycheck.contribution[year]`. Accounts have totals too, like
`income.salary[year]`.

### Checks

```
check assets.cash >= 0
check assets.retirement.beth == 24_500 @ 2026-12-31
```

Checks are checked after entries fire on their days, or every day. A failing
check doesn't stop the simulation: saldo reports it after the output, and
exits with status 3.

### Expressions

| Form | Description |
|------|-------------|
| `1_000`, `0.05` | Numeric literals (underscores ignored) |
| `6.2%` | Percentage: divides by 100 |
| `24_500/year` | Rate: an amount per `day`, `week`, `month`, `quarter` or `year` |
| `24_500[year]` | Limit: an amount each period, filled over its firings |
| `true`, `false` | Boolean literals |
| `assets.cash`, `assets.retirement` | Account or group balance |
| `salary`, `loans.rate` | Value |
| `contribution[year]`, `paycheck.contribution[year]` | Tag total |
| `income.salary[year]` | Account total |
| `a + b`, `a - b`, `a * b`, `a / b` | Arithmetic |
| `1.06^(1/12)` | Power of plain numbers; binds tightest and groups to the right, so `-2^2 == -4` |
| `a == b`, `a != b`, `a < b`, `a <= b`, `a > b`, `a >= b` | Comparison (can't be chained) |
| `a and b`, `a or b`, `not a` | Logical operators |
| `if c then a else b` | Conditional |
| `min(a, b)`, `max(a, b)` | Built-in functions |
| `fill(max_401k - other[year])` | What's left of a limit or total, split over the firings left in the period |
| `abs(x)`, `floor(x)`, `ceil(x)`, `round(x)` | Built-in functions |
| `normal(mean, sd)`, `lognormal(mean, sd)`, `uniform(lo, hi)` | A random draw |
| `chance(4%)` | `true` with probability 4% |
| `net(gross, 0.3)` | User-defined function call |

Expressions are numbers or bools. saldo checks that each is used where it's
expected (amounts are numbers, checks and `if` conditions are bools, and a
value's arms are all one or the other) before it simulates anything.

Amounts moved and opening balances are rounded to cents, and `round(x)` rounds
to a whole number. Both use round-half-to-even (`round(2.5) == 2`), as hledger
and beancount do. Entries that move no money are left out of the output.

### Random draws

```
let market = normal(0.5%, 4%)
let salary = normal(100_000, 5_000)/year

entry repair "Car repair" @ last {
  assets.cash -> expenses.car : if chance(4%) then uniform(300, 2_500) else 0
}
```

A draw changes once per period of what it is: `salary` once a year, spread
over the year's paychecks, and a plain number like `market` each day it's
worked out, so once per firing. Everything that reads a value on the same day
sees the same draw, and a distribution written in two places draws in each. A
draw depends only on the seed, where it's written and when, so `--seed`
repeats a run whatever `--from` is.

### Functions

```
fn net(gross, rate) {
  let tax = gross * rate
  gross - tax
}
```

Functions can use their parameters, their `let`s and values, but not balances
or totals, and may not recurse. The final expression is the return value.

### Modules

```
mod loans
mod pay {
  let salary = 95_000/year
}
```

Modules organize a model. `mod loans` reads `loans.saldo`, and `mod pay { … }`
is written in place. A module's declarations are named by their path, like
`pay.salary`; inside a module, names are looked up in the module and then at
the top of the model.

```
fn loan(debt, rate, payment) {
  mod {
    entry repay @ last {
      debt        -> expenses.interest : debt * rate
      assets.cash -> debt              : min(payment, debt + debt * rate)
    }
  }
}

mod car = loan(debt: liabilities.loans.car, rate: 6%/year, payment: 600)
```

A function that ends with `mod { … }` makes a module, so `mod car = loan(…)`
gives `car.repay`, moving money out of `liabilities.loans.car`. Any call can
give its arguments by name.

### Scenarios

```
scenario prepay {
  let loans.car.payment = 1_200        // replaces loans.car.payment
}

scenario daycare {
  drop childcare.nanny @ 2027-09-01..  // stops it firing from September

  entry childcare.daycare "Daycare" @ 1st & 2027-09-01.. {
    assets.cash -> expenses.childcare.daycare : 2_100
  }
}
```

A scenario changes the model: `--scenario prepay` runs the model with it. A
declaration in a scenario is named by its path into the model's modules, and
replaces the declaration there, or adds it if there's none. In a value that
replaces another, its own name reads the one it replaces, like
`let pay.salary = pay.salary * 80%`. `drop` stops an entry, or a module's
entries and checks, on some days or all of them. A scenario can be declared
in modules, where it changes what's in them, and blocks with one name are one
scenario, so a move can change `housing` and `tax` each in its own file. With
the same seed, a scenario draws what the model draws, except where it changes
it, so `--scenario base,prepay --runs 1000` compares the two run by run.

## Building

```
cargo build --release
```

Requires Rust 1.91+.

## Tree-sitter grammar

The `treesitter/` directory contains a Tree-sitter grammar for the saldo language, with bindings for Rust, Node.js, Python, Go, Swift, and C.
