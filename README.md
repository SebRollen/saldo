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
```

- `--from` / `--to` — inclusive date range to simulate
- `--format ledger` (default) — outputs double-entry ledger transactions
- `--format csv` — outputs the daily balance of every account as CSV

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

Values are named numbers. A value can be a constant, or take different values
on different days, each arm covering a set of days and the first that covers a
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

On a day several entries fire, an entry fires after the entries that move what
it reads. When two entries each read what the other moves, like the interest
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

Checks are checked after entries fire on their days, or every day. Simulation
stops with an error if one fails.

### Expressions

| Form | Description |
|------|-------------|
| `1_000`, `0.05` | Numeric literals (underscores ignored) |
| `6.2%` | Percentage: divides by 100 |
| `24_500/year` | Rate: an amount per `day`, `week`, `month`, `quarter` or `year` |
| `true`, `false` | Boolean literals |
| `assets.cash`, `assets.retirement` | Account or group balance |
| `salary`, `loans.rate` | Value |
| `contribution[year]`, `paycheck.contribution[year]` | Tag total |
| `income.salary[year]` | Account total |
| `a + b`, `a - b`, `a * b`, `a / b` | Arithmetic |
| `a == b`, `a != b`, `a < b`, `a <= b`, `a > b`, `a >= b` | Comparison (can't be chained) |
| `a and b`, `a or b`, `not a` | Logical operators |
| `if c then a else b` | Conditional |
| `min(a, b)`, `max(a, b)` | Built-in functions |
| `fill(24_500/year)` | What's left of a period's amount, split over the firings left in it |
| `abs(x)`, `floor(x)`, `ceil(x)`, `round(x)` | Built-in functions |
| `net(gross, 0.3)` | User-defined function call |

Values are numbers or bools. saldo checks that each is used where it's
expected (amounts and values are numbers, checks and `if` conditions are
bools) before it simulates anything.

Amounts moved and opening balances are rounded to cents, and `round(x)` rounds
to a whole number. Both use round-half-to-even (`round(2.5) == 2`), as hledger
and beancount do. Entries that move no money are left out of the output.

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

## Building

```
cargo build --release
```

Requires Rust 1.91+.

## Tree-sitter grammar

The `treesitter/` directory contains a Tree-sitter grammar for the saldo language, with bindings for Rust, Node.js, Python, Go, Swift, and C.
