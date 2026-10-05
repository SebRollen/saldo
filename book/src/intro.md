# Introduction

## What is saldo?

`saldo` is a domain-specific programming language (DSL) and command-line
interface (CLI) for creating financial forecasts.

A model is made of a few kinds of things:

- **Accounts** hold money: `assets.cash`, `liabilities.loan`.
- **Sets of days** say when things happen: `15th | last`, `fri`, `jul 2026..`.
- **Values** are numbers that can change from day to day: a salary, an
  interest rate, a contribution limit.
- **Entries** move money between accounts on a set of days.
- **Checks** are conditions that must hold, like having money in the bank.

Functions and modules organize the rest.

```
balances @ 2026-01-01 { assets.cash = 5_000 }

let salary = 95_000/year

entry paycheck "Paycheck" @ 15th | last {
  income.salary -> assets.cash : salary
}

entry rent "Rent" @ 1st {
  assets.cash -> expenses.rent : 2_000
}

check assets.cash >= 0
```

saldo simulates the model day by day and prints the transactions it makes in
ledger format, for tools like hledger.
