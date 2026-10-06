# Random distributions

Status: designed, not started. Revised for the redesigned language: values
that depend only on the date, rates and limits, full names, modules and `for`.

Let a model describe uncertain amounts, like a grocery bill, car repairs,
inflation or market returns, as distributions. A seed fixes every draw, so it
reproduces a run. Monte Carlo (`--runs N`) comes later, and nothing here
should get in its way.

## What users write

```
let salary    = normal(120_000, 5_000)/year     // drawn once a year
let inflation = normal(3%, 1%)                  // drawn on each day it's read…
let prices {
  2026.. => 1 compounding inflation every year  // …so each step draws its own
}
let market = normal(0.5%, 4.3%)                 // one draw a day, for all its readers

entry groceries "Groceries" @ sun {
  assets.cash -> expenses.groceries : max(0, normal(185, 40)) * prices
}

entry car_repair "Car repair" @ last {
  assets.cash -> expenses.car : if chance(4%) then uniform(300, 2_500) else 0
}

entry returns "Investment returns" @ last {
  for a in assets.retirement, assets.college {
    income.investments -> a : a * market        // the same return for every account
  }
}
```

```
saldo model.saldo --from 2026-01-01 --to 2026-12-31 --seed 7
```

## Decisions

### A draw is a value of the date and the seed

Values depend on nothing but the date, so saldo can work one out for any day,
and does: `spread` works an amount out again for each period that rolls into a
firing, and `param_on` retakes a growing arm's steps from its start
(`src/eval.rs:953`, `:504`). A draw keeps that promise if it's a pure function
of `(seed, site, period)`: given the seed, it depends on nothing but the date.
A random stream couldn't: each evaluation would draw again, so a value would
differ between today and a rolled-in period's second look at it.

Keyed draws also mean:

- `--from` doesn't change the draws, as it doesn't change the paychecks.
- Adding an unrelated entry or value, or an `if` that skips a branch, doesn't
  shift any other draw.
- Two versions of a model see the same draws wherever they share sites, so the
  difference between them comes from the change and not from noise. Every
  distribution is the inverse CDF of one uniform number, so changing a draw's
  arguments (a mean of 190 instead of 185) keeps its quantile in each period:
  a run that was a bad month for groceries still is.

### A draw's kind says how often it changes

The units pass already gives every expression a kind (`src/units.rs:24`). A
draw changes once per period of the kind it's written as:

| Draw | Kind | Drawn |
|------|------|-------|
| `normal(185, 40)` | a number | each day it's evaluated: once per firing in an entry, once per step in growth |
| `normal(800, 100)/month`, `normal(800/month, 100/month)` | an amount per month | once a month |
| `normal(10_000, 2_000)[year]` | an amount each year | once a year |
| `chance(4%)` | a bool | each day it's evaluated |

- A rate is drawn once per period and spread over the firings in it, which
  keeps the variance written: a weekly entry moving `normal(800, 100)/month`
  moves parts of one draw, and each month comes to its draw. Drawing each
  firing's share separately would give a monthly sd of about 48, not 100.
- A limit is drawn once per period, which is what filling needs: what's left of
  this year's draw, over the firings left.
- Plain draws can go in values. (The first version of this plan made that an
  error.) A value is worked out each day, so a plain draw in one changes each
  day, which is right wherever it's read once a period: a growth step reads
  `inflation` on the step's day, and a month-end entry reads `market` once a
  month.

The period comes from the draw's arguments or a unit applied directly to it.
A draw that changes more often than the rate or limit it ends up in is an
error, whether in the same expression or through values:

```
let x = normal(100, 10)

entry food {
  assets.cash -> expenses.food : x/month
}
```

> `x` is drawn every day, but this moves it per month, so each day would move
> part of a different draw; draw it per month: `normal(100, 10)/month`

The units pass tracks, beside each expression's kind, the shortest period its
draws change over. A rate or limit per `U` can't hold draws that change more
often than `U`. This also covers `if`, `min` and `max` picking between a plain
draw and a rate.

### Naming a draw shares it

A draw belongs to where it's written, and everything that reads a value on the
same day sees the same draw. So:

- To share a draw, name it: one `market` return for every account, or a `let`
  in an entry for two moves of the same firing.
- To draw independently, write the distribution in each place. One written in
  a `for` body is drawn separately for each account the loop goes through.
- Each instance of a function that makes a module (`mod car = loan(…)`) draws
  for itself, since its entries and values have their own full names.
- To reuse a distribution's parameters but not its draws, name the parameters:
  `normal(grocery_mean, 40)`.

### Site: where a draw is written

Every entry and value has a full name now, so a site is built from names, not
from declaration order:

- **In a value**: its full name, like `spending.groceries`; the arm, if it has
  arms, by index; whether the draw is in the arm's value or in its growth's
  rate; and which draw it is in that expression, in tree order.
- **In an entry**: its full name, like `loans.car.repay`; the statement's
  position in the body as written, as a path of indices through `if` branches
  and `for` bodies; the account each enclosing `for` variable stands for; and
  which draw it is in the expression.

The draw's arguments aren't part of the site (see above). Editing a value's
arms or an entry's statements may change its own draws, and renaming it
changes them; nothing else does.

The resolver flattens loops (`src/resolver.rs:1080`), losing where statements
came from, so it records each statement's position and loop bindings as it
expands them.

### Where draws can't be written

- **`balances` blocks**: opening balances and totals so far are facts.
- **Checks**: a check can read values that draw, but a draw written in one
  would make the condition itself random.
- **Functions that return a value**: they're called from many places, and a
  draw in one would be shared by every call on the same day. Pass the draw in
  as an argument, or name it as a value. Functions that make modules can draw,
  since their entries and values get the instance's names.

### Periods that roll into a firing

The first version of this plan made a draw that changes more often than its
entry fires an error ("drawn per week, but `Groceries` fires monthly"). Values
have made that unnecessary. When an entry moves a rate for periods with no
firing of their own, `spread` works the amount out again on each period's last
day, if it depends on nothing but the date. A draw depends on nothing but the
date and the seed, so `dated` (`src/eval.rs:1306`) counts it, and each week
that rolls in gets its own weekly draw. A monthly entry moving
`normal(185, 40)/week` then has a monthly sd of about 40 · √4.3 ≈ 83, as
independent weeks should.

An amount that also reads balances gets today's value for every period that
rolls in, and `check_carried` (`src/eval.rs:1017`) errors if a value it reads
changes during them. Draws count as changes there: a draw in the amount, or in
a value it reads, that changes more often than the entry fires gives the same
error ("fire it at least once a week").

### Seed

- **The default is random**, so adding a distribution visibly varies the
  output. When the model draws, the CLI prints the seed to stderr with the
  warnings: `note: drew with seed 8812047; pass --seed 8812047 to reproduce`.
  Models that don't draw print nothing extra.
- **`--seed N`** fixes it. N is a `u64`.
- **The library stays deterministic.** `RunOpts` gets a required `seed: u64`,
  and `main.rs` picks a random one when `--seed` is missing. std's
  `RandomState` is seeded from the OS, so this needs no new dependency. Tests
  pass fixed seeds.
- `Output` says whether the model drew anything, so the CLI knows whether to
  print the note.
- The `Seed` structs that exist today (`src/compile.rs:43`,
  `src/resolver.rs:47`) are the totals a `balances` block sets. Rename them,
  say to `TotalSoFar`, so that "seed" means one thing.

### Distributions in v1

- `normal(mean, sd)`
- `uniform(lo, hi)`
- `chance(p)`: true with probability `p`, a plain number. It returns a bool,
  which `typecheck.rs:204` doesn't expect of a built-in yet. Values are
  numbers, so share an event by naming what it picks:
  `let repair = if chance(4%) then uniform(300, 2_500) else 0`.

A draw's arguments combine as `min`'s and `max`'s do: the same kind, or plain
numbers that take on the other's unit. Nothing is clamped: users write
`max(0, …)` themselves.

Later: `lognormal(mean, sd)`, taking the mean and sd of the result rather than
of its log (returns that can't lose more than everything), and
`triangular(min, mode, max)`. Each must also be the inverse CDF of one uniform
number.

## Keeping Monte Carlo possible

- **Mix the seed in at simulation time.** Sites are hashed at compile time, but
  the seed goes to `Program::simulate`, so Monte Carlo compiles once and
  simulates N times.
- **Each run is reproducible on its own.** Run *i* uses seed `mix(master, i)`,
  and a single run uses its seed directly. A report can say "cash went negative
  in 31 of 1000 runs, e.g. `--seed 4471…`", and that seed replays the run as an
  ordinary ledger.
- **No RNG state in `State`.** What `State` caches (each day's values, growth
  steps taken) is fixed by the seed, and lives for one run.
- **Keep failed checks apart from model errors.** A failing check stops the
  simulation with a `Diagnostic`, as an error does. Monte Carlo counts failed
  checks ("cash went negative in 31% of runs, first in July") but must stop on
  a broken model. There's nothing to change yet, but don't merge these paths
  any further. With draws, errors like dividing by zero can depend on the seed
  too.

## Implementation sketch

1. **Names and types**: `Builtin::{Normal, Uniform, Chance}`
   (`src/compile.rs:210`). In `typecheck.rs`, `chance` takes a number and
   returns a bool.
2. **Resolver**: reject draws in `balances` blocks, checks and the bodies of
   functions that return a value. Record each entry statement's position and
   loop bindings while expanding `for` loops.
3. **Compile**: `ExprKind::Draw(dist, args, slot)` with
   `Program::draws: Vec<Draw { key: u64, period: Option<TimeUnit>, span }>`,
   the way `fill` becomes `ExprKind::Fill` with a slot in `Program::fills`.
   Hash each site into `key` here.
4. **Units pass** (`src/units.rs`): work out each draw's kind and set its
   `period`; track the shortest period an expression's draws change over;
   error where a rate or limit holds a draw that changes more often.
5. **Evaluation** (`src/eval.rs`): `State` holds the seed. A draw evaluated for
   date `d`, which may not be today (`eval_num_at`), mixes `(seed, key, n)`,
   where `n` numbers `d`'s day, or the period of `d` for a rate or limit. It
   takes a uniform number in [0, 1) from the top 53 bits, transforms it, and
   converts it with `Decimal::from_f64`. `dated` counts draws;
   `check_carried` treats a draw that changes during the rolled-in periods as
   a change.
6. **Hashing**: hand-rolled SplitMix64, not std's `DefaultHasher`, whose
   output isn't guaranteed across Rust releases. Hash names into sites with a
   stable hash such as FNV-1a.
7. **Normal**: inverse CDF (Acklam, or Wichura's AS241), one uniform per draw.
   `f64::ln` comes from the platform's libm and can differ in the last bit
   between platforms, which rounding to cents almost always hides. Use the
   `libm` crate if the same seed must give identical output everywhere.
8. **CLI**: `--seed`, a random default, and the stderr note.
9. **Docs**: a book chapter on distributions (how often each kind draws,
   sharing by naming, seeds), rows in `fns.md`'s built-ins table, the seed in
   `simulation.md`, and the README. Tree-sitter needs no change, since
   built-ins are ordinary calls.

## Open questions

- **A number that holds for a period.** A plain draw changes every day, so a
  value like this year's inflation, read by a growth step in January and
  another in April, gets two draws. Rates and limits hold for a period, but
  growth rates are plain numbers. Options: let `compounding` take a rate per
  its step (`compounding normal(3%, 1%)/year every year`), or wait and see
  whether it comes up.

## Deferred

- **Compounding in spreads.** 12% a year is about 0.949% a month compounded,
  not 1%. Rates are split linearly today, interest included, and draws follow
  suit. Fix both together later.
- **Monte Carlo** (`--runs N`), with aggregate output: balance percentiles and
  how often checks fail.
- **A seed in the model file.**
- **Correlation** other than sharing one draw, like stocks and bonds that tend
  to move together.
- **Events that last**, like a job loss for six months. A draw can't remember
  an earlier one, since values have no state.
- Other distributions, see above.
