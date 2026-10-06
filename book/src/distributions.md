# Random draws

Some amounts can't be known in advance, like a grocery bill, a car repair,
inflation or a market's returns. A model can describe them as draws from a
distribution:

```
let inflation = normal(3%, 1%)
let prices {
  2026.. => 1 compounding inflation every year
}

entry groceries "Groceries" @ sun {
  assets.cash -> expenses.groceries : max(0, normal(185, 40)) * prices
}

entry car_repair "Car repair" @ last {
  assets.cash -> expenses.car : if chance(4%) then uniform(300, 2_500) else 0
}
```

Every run draws from a seed, a whole number. The same seed draws the same
numbers, so it repeats a run exactly (see [Seeds](#seeds)).

## Distributions

| Function | Draws |
|----------|-------|
| `normal(mean, sd)` | a number from a normal distribution with that mean and standard deviation |
| `uniform(lo, hi)` | a number from `lo` up to `hi`, every one as likely as any other |
| `chance(p)` | `true` with probability `p`, like `4%`, and `false` otherwise |

`normal` and `uniform` take two numbers of the same kind, like two amounts per
month, or a plain number that takes on the other's kind, as with `min` and
`max`. `chance` takes a plain number between 0 and 1.

Nothing is clamped. A normal distribution can draw a negative grocery bill, so
write `max(0, normal(185, 40))` where that matters.

## How often a draw changes

A draw changes once per period of what it is:

| Draw | Is | Drawn |
|------|----|-------|
| `normal(185, 40)` | a number | each day it's worked out: once per firing in an entry |
| `normal(800, 100)/month` | an amount per month | once a month |
| `normal(800/month, 100/month)` | an amount per month | once a month |
| `normal(10_000, 2_000)[year]` | an amount each year | once a year |
| `chance(4%)` | a bool | each day it's worked out |

An entry spreads an amount per month over the month's firings (see
[Entries](./entries.md#rates)), so a weekly entry moving
`normal(800, 100)/month` moves parts of one draw, and each month comes to
exactly its draw. A limit, like `normal(10_000, 2_000)[year]`, is drawn once a
year and filled over the year's firings.

A `/month` or `[month]` applies to every draw written in the expression before
it, so this draws a salary's ups and downs once a year:

```
let salary = 100_000/year

entry pay "Paycheck" @ 15th | last {
  income.salary -> assets.cash : salary * normal(1, 10%)/year
}
```

A plain number is drawn each day it's worked out. A growth step works its rate
out on the step's day, so `prices` above draws its inflation once a year, on
January 1st. An entry works its amounts out when it fires, so `groceries`
draws a new bill each Sunday.

When an entry moves an amount per period for periods with no firing of their
own, each of those periods gets its own draw. A monthly entry moving
`normal(185, 40)/week` moves four or five weekly draws, one for each week of
the month:

```
entry groceries "Groceries" @ last {
  assets.cash -> expenses.groceries : normal(185, 40)/week
}
```

An amount that reads balances or totals gets only today's draw for those
periods, which would be wrong, so that's an error: fire the entry at least
once each period it draws for, or draw for a longer period.

An amount per period can't hold a draw that changes more often than its period,
or the firings in a period would each move part of a different draw:

```
let bill = normal(100, 10)

entry food "Food" {
  assets.cash -> expenses.food : bill/month   // an error: `bill` is drawn every day
}
```

Draw it per month instead: `let bill = normal(100, 10)/month`.

## Sharing a draw

A draw belongs to where it's written, and everything that reads a value on the
same day sees the same draw. So a value shares one draw with everything that
reads it, and a distribution written in several places draws in each:

```
let market = normal(0.5%, 4%)

balances @ 2026-01-01 {
  assets.retirement.alex = 50_000
  assets.retirement.beth = 50_000
}

entry returns "Investment returns" @ last {
  for a in assets.retirement {
    income.investments -> a : a * market      // the same return for both
  }
}
```

Written inside the loop, as `a * normal(0.5%, 4%)`, it would be drawn
separately for each account. In the same way:

- A `let` in an entry draws once per firing for every move that uses it.
- Each module that a function makes, like `mod car = loan(…)`, draws for
  itself.
- To use a distribution's numbers in several places but draw separately in
  each, name the numbers: `normal(grocery_mean, 40)`.

## Where draws can't go

- **Checks** and **`balances` blocks**, which say what must hold and what
  was. A check can read a value that draws.
- **Functions that return a value**, which are called from many places. Pass
  the draw in as an argument instead.
- **The arguments and `let`s of a function that makes a module**, which are
  copied wherever the module uses them, so they'd be drawn separately in each
  place. Name the draw with a `let` and pass the name, or draw inside the
  module.

## Seeds

Without `--seed`, saldo picks a seed at random and prints it:

```
❯ saldo budget.saldo --from 2026-01-01 --to 2026-12-31
…
note: drew with seed 8812047; pass --seed 8812047 to reproduce
```

`--seed 8812047` then repeats that run. An error during a run that draws
names the seed too, since what went wrong may depend on what was drawn.

A draw depends only on the seed, where it's written and the period it's drawn
for, so:

- Where the report starts doesn't change the draws, as it doesn't change the
  paychecks.
- Adding an entry or a value doesn't change the draws of the others. Editing an
  entry or a value may change its own draws, and renaming it does.
- Changing a distribution's numbers keeps its draws in step: with the same
  seed, `normal(190, 40)` draws 5 more than `normal(185, 40)` every time, so
  comparing two versions of a model shows what the change does, not chance.
