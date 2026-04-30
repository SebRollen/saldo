# Asserts

An assertion is a condition that must hold true on certain days. If the
condition evaluates to false the simulation stops and reports the failure.
Assertions are how you express financial constraints and goals.

## Syntax

```
assert [<schedule>] <boolean-expression>
```

Without a schedule, an assertion is checked every day of the simulation.
With a schedule, it is checked only on days that match.

## Daily assertions

```
assert Assets:Cash >= 0
assert jim_paycheck.retirement_contribution.ytd <= 24_500
```

These fire every day. The first ensures the cash account never goes
negative. The second ensures a named leg never exceeds a limit.

## Scheduled assertions

Any schedule expression can precede the condition:

```
assert quarterly Assets:Retirement >= 0
assert monthly on the last day Assets:Cash >= 10_000
assert every friday Liabilities:AccruedInterest >= 0
```

## Date-specific assertions

A single date (or a list of dates) acts as a schedule:

```
assert 2026-12-31 Assets:Retirement:Seb  == 24_500
assert 2026-12-31 Assets:Retirement:Jess == 24_500
```

## Expressions

Assertion expressions support the same operators as entry amounts:

| Operator | Meaning |
|----------|---------|
| `<`, `<=` | Less than, at most |
| `>`, `>=` | Greater than, at least |
| `==` | Equal |
| `if … then … else …` | Conditional (both `then` and `else` are required) |
| `min()`, `max()` | Built-in functions |

Account references, param names, and aggregation suffixes (`.ytd`, `.qtd`,
`.mtd`) all work inside assertion expressions.

## Examples

```
// Cash never goes negative
assert Assets:Cash >= 0

// 401(k) contribution limit not breached
assert jim_paycheck.retirement_contribution.ytd <= max_401k

// Target retirement balance hit by a specific date
assert 2026-12-31 Assets:Retirement:Beth == 24_500

// Sanity-check every quarter
assert quarterly Assets:Retirement:Seb >= 0
```
