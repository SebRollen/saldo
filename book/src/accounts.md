# Accounts

An account is a named balance that the simulator tracks over time.

## Names

An account's name is a path of words joined by dots, starting with its kind:

```
entry paycheck @ last {
  income.salary -> assets.cash : 8_000
  assets.cash   -> assets.retirement.alex : 1_000
}
```

| Kind | Holds |
|------|-------|
| `assets` | Things you own: cash, retirement accounts, savings |
| `liabilities` | Things you owe: loans, accrued interest, taxes payable |
| `income` | Where money comes from |
| `expenses` | Where money goes |
| `equity` | What's left over |

Accounts don't need declaring. An account exists because an entry moves money
into or out of it, or because it has an opening balance. Reading an account
that nothing moves money into or out of is an error, which catches typos:

```
check assets.csh >= 0   // error: no entry or balance moves money into or out of `assets.csh`
```

Two accounts next to each other whose names differ by a letter, like
`assets.retirement` and `assets.retirment`, get a warning.

In ledger output, each word is capitalized and `_` starts a new word, so
`liabilities.taxes_payable` is `Liabilities:TaxesPayable`.

## Groups

A path with accounts under it is a group. Reading a group gives the sum of
the accounts in it:

```
let monthly_return = 0.5%

entry returns @ last {
  income.investments -> assets.retirement.alex : assets.retirement.alex * monthly_return
  income.investments -> assets.retirement.sam  : assets.retirement.sam * monthly_return
}

check assets.retirement >= 0
```

`assets.retirement` is the sum of `assets.retirement.alex` and
`assets.retirement.sam`, and `assets` is every asset. An account either holds
money or is a group: moving money into `assets.cash` and into
`assets.cash.checking` is an error.

## Kinds

Every account reads positive the way you'd talk about it: what you own, what
you owe, what you've earned, what you've spent.

```
balances @ 2026-01-01 {
  assets.cash      = 1_000
  liabilities.loan = 3_000   // you owe 3,000
}

entry pay @ 2026-01-15 { assets.cash -> liabilities.loan : 500 }

check liabilities.loan == 2_500 @ 2026-01-31
check assets - liabilities == -2_000 @ 2026-01-31
```

Ledger output keeps ledger's signs, which hledger and the like expect:
liabilities, income and equity are negative there.

## Opening balances

Without an opening balance, an account starts at zero and is available for the
entire simulation. `balances` gives accounts their balances on a date:

```
balances @ 2025-01-01 {
  assets.cash      = 12_500
  liabilities.loan = 450_000
}

balances @ 2024-07-01 {
  assets.retirement.alex = 87_340.22
}
```

The balances are amounts as the kind reads them, so a loan's is what's owed.
Each can be an expression, evaluated on its date: it can use values, and
accounts that are already open, but not accounts that open later.
An account opens once, so it can only be in one `balances` block.

### Referencing accounts before they open

Reading or moving money into an account before its opening date is an error:

```
balances @ 2025-06-01 { assets.cash = 1_000 }

// This entry fires in January, before `assets.cash` opens, and will error:
entry paycheck @ last {
  income.salary -> assets.cash : 500
}
```

To avoid the error, make sure entries start no earlier than the accounts they
use, or set the opening date early enough to cover the simulation. A group
counts accounts that haven't opened yet as zero.

### Simulation start after opening date

If your simulation starts after an account's opening date, saldo warms up the
simulation from the opening date. Entries that fire during the warm-up are
processed normally; only their output is left out. The opening-balances entry
in ledger output shows the balances at the start of your simulation, after the
warm-up.

```
balances @ 2024-01-01 { assets.cash = 1_000 }

entry paycheck @ last {
  income.salary -> assets.cash : 500
}

// Running from 2025-01-01: `assets.cash` opens at 1,000 on 2024-01-01, then
// 12 monthly paychecks fire during warm-up, so the opening balance shown is
// 7,000.
```

## Totals

`[year]`, `[quarter]` and `[month]` after an account give what has moved into
it so far this period, read as its kind reads:

```
check income.salary[year] <= 200_000
```

They count every entry that fired today before the one reading them. See
[Entries](./entries.md#tags-and-totals) for totals of particular moves.
