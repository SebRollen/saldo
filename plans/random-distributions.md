# Random distributions

Status: designed, not started.

Let a model describe uncertain amounts, like a grocery bill, car repairs or
market returns, as distributions. Each firing or period gets its own draw, and a
seed reproduces a run. Monte Carlo (`--runs N`) comes later, and nothing here
should get in its way.

## What users write

```
param salary = normal(120_000, 5_000) per year     // one draw per year

entry weekly "Groceries" {
  Expenses:Groceries = max(0, normal(185, 40))     // a new draw every firing
  Assets:Cash
}

entry monthly "Car repair" {
  Expenses:Car = if chance(4%) then uniform(300, 2_500) else 0
  Assets:Cash
}
```

```
saldo model.saldo --from 2026-01-01 --to 2026-12-31 --seed 7
```

## Decisions

### Draws are keyed, not streamed

Each draw is a pure function of `(seed, site, occurrence)`. No RNG state is
consumed in evaluation order. With a stream, every draw would depend on what was
drawn before it:

- Changing `--from` would change the draws, because the warm-up consumes some.
  Keyed draws keep the promise in `entries.md` that you see the same paychecks
  whatever `--from` you choose.
- Adding an unrelated entry, an `if` that short-circuits, or params evaluated a
  second time on a day an account opens (`src/eval.rs:267`) would shift
  everything after it.
- Two scenarios of the same model (pay the loan off faster or not) see the same
  grocery bills, so the difference between them comes from the decision and not
  from noise.

**Site**: a name for where the draw is written that survives edits elsewhere in
the model.

- In a posting: the entry's alias, or its label if it has none (plus which entry
  with that label it is, if labels repeat), the posting's index in source order,
  and which draw it is within the posting's expression, in tree order. Don't use
  the resolver's synthetic `$N` entry keys (`src/resolver.rs:20`): they're
  declaration indices, so adding an entry would change the draws of every entry
  after it.
- In a param: the param's name, the interval's `from` date (if it has
  intervals), and which draw it is within that expression.

Editing an entry's own postings may change that entry's draws, which is fine.

**Occurrence**: the day number for a plain draw in a posting, or
`unit.period_number(date)` for a rate drawn per `unit` (`src/ast/mod.rs`).

### The units decide how often a draw changes

| Where | Draw is | Drawn |
|-------|---------|-------|
| posting | plain number, `normal(185, 40)` | once per firing |
| posting or param | rate, `normal(800, 100) per month` | once per period of its unit |
| param | plain number | error: a param has no firing, so it would change every day |
| fn body, assertion, opening balance | anything | error |

A rate is drawn once per period and spread over the entry's firings by the
existing spreading. This keeps the variance the user wrote. If each firing drew
its own share of `normal(800, 100) per month`, a weekly entry's monthly total
would have an sd of about 48, not 100.

There's no override like `drawn yearly`. Anything that should hold over a
period is written as a rate. To reuse a distribution across postings, put its
mean and sd in params: `normal(grocery_mean, 40)`.

Functions stay pure, as documented. Users can pass a draw in as an argument
instead.

A draw's period comes from its arguments, if they're rates
(`normal(800 per month, 100 per month)`), or from a `per` applied directly to it
(`normal(800, 100) per month`). A plain draw that ends up inside a rate some
other way, like `normal(800, 100) * (1.1 per month)` or an `if` that picks
between it and a rate, is ambiguous. Reject it with a hint to write it as a rate.

### A draw's period can't be shorter than the gap between firings

A monthly entry posting `normal(185, 40) per week` covers several weeks with
each firing, because weeks without a firing roll into the next one. One weekly
draw times 4.3 weeks gives the month's total an sd of about 172. Independent
weekly draws would give about 83. In v1 this is an error: "drawn per week, but
`Groceries` fires monthly; draw per month instead". Check it at the point where
a spread rolls skipped periods into a firing (`share.carried > 0`) and its
expression contains a draw at that unit. A static check against the schedule
would be nicer, if the schedule can answer "does this fire in every period?".

A draw coarser than the firings is fine: that's the per-year draw in a monthly
entry.

### Seed

- **The default is random**, so adding a distribution visibly varies the
  output. When the model has draws, the CLI prints the seed to stderr next to
  the warnings: `note: drew with seed 8812047; pass --seed 8812047 to
  reproduce`. Models without draws print nothing extra.
- **`--seed N`** fixes it. N is a `u64`.
- **The library stays deterministic.** `RunOpts` gets a required `seed: u64`,
  and `main.rs` picks a random one when `--seed` is missing. std's
  `RandomState` is seeded from the OS, so this needs no new dependency. Tests
  pass fixed seeds.
- `Output` has to tell the CLI whether the model drew anything, so it knows
  whether to print the note.

### Distributions in v1

- `normal(mean, sd)`
- `uniform(lo, hi)`
- `chance(p)` returns a bool, for events like `if chance(4%) then … else 0`.
  `src/typecheck.rs:171` assumes every built-in returns a number, so it needs a
  case for this.

A draw's arguments must be the same kind, like `min` and `max`. Nothing is
clamped automatically: users write `max(0, …)` themselves.

Later: `lognormal(mean, sd)`, taking the mean and sd of the result rather than
of its log, and `triangular(min, mode, max)`.

## Keeping Monte Carlo possible

- **Mix the seed in at simulation time.** Site keys can be hashed at compile
  time, but the seed goes to `simulate`, not into `Program`. Monte Carlo then
  compiles once and simulates N times.
- **Each run is reproducible on its own.** Run *i* uses seed `mix(master, i)`,
  and a single run uses its seed directly. A Monte Carlo report can say "cash
  went negative in 31 of 1000 runs, e.g. `--seed 4471…`", and that seed replays
  the run as an ordinary ledger.
- **No RNG state in `State`.** A draw depends only on seed, site and
  occurrence.
- **Keep assertion failures separate from model errors.** `simulate` returns a
  single `Diagnostic` for both today. Monte Carlo will need to tell "this run
  broke an assertion" (an outcome to count) from "this model is broken". There's
  nothing to change yet, but don't merge these paths any further. Note that with
  draws, model errors like division by zero can also depend on the seed.

## Implementation sketch

1. **Names and types**: add `normal`, `uniform` and `chance` to `BUILTINS`
   (`src/eval.rs:829`) and `Builtin`. In `typecheck.rs`, `chance` returns a bool
   and its argument is a number.
2. **Units pass** (`src/units.rs`): work out each draw's kind from its arguments
   or a `per` applied directly to it, and enforce the placement rules above.
   Rewrite draws to `ExprKind::Draw(dist, args, site)`, with a
   `Program::draws: Vec<DrawSite { key: u64, unit: Option<TimeUnit>, span }>`,
   the same way `fill` becomes `ExprKind::Fill` with a slot in `Program::fills`.
   Postings record whether their spread contains a draw at the spread's unit.
3. **Evaluation** (`src/eval.rs`): `State` holds the seed. Evaluating a draw
   mixes `(seed, site.key, occurrence)` into a `u64`, takes a uniform number in
   [0, 1) from its top 53 bits, transforms it, and converts it with
   `Decimal::from_f64`. `spread` errors when `share.carried > 0` and the posting
   draws at that unit.
4. **Hashing**: hand-rolled SplitMix64. Not std's `DefaultHasher`, whose output
   isn't guaranteed across Rust releases. Hash site names to `u64` with a
   stable hash such as FNV-1a. If a distribution needs more than one uniform
   number, mix in a counter.
5. **Normal**: inverse CDF (Acklam or Wichura AS241), one uniform per draw.
   `f64::ln` comes from the platform's libm and can differ in the last bit
   between platforms. Rounding to cents hides that almost always, but not
   always. Use the `libm` crate if the same seed must give identical output
   everywhere.
6. **CLI**: `--seed`, a random default, and the stderr note.
7. **Docs**: a book page on distributions (draw frequency, seeds, the
   finer-than-firing error), plus rows in the README's expression table and
   `params.md`. Tree-sitter needs no change, since built-ins are ordinary calls.

## Deferred

- **Compounding.** 12% a year is about 0.949% a month compounded, not 1%. Rates
  are split linearly today, interest included, and draws follow suit. Fix both
  together later.
- **Monte Carlo** (`--runs N`), with aggregate output: balance percentiles and
  how often assertions fail.
- **A `seed` declaration in the model file.**
- Other distributions, see above.
