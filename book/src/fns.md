# Functions

Functions name and reuse a computation across values, entries and checks.
They take arguments and return a value.

## Syntax

```
fn <name>(<parameter>, ...) {
  [let <name> = <expression>]
  ...
  <expression>
}
```

The final expression is the return value.

## Defining a function

```
fn double(x) { x * 2 }

fn net(gross, rate) {
  let tax = gross * rate
  gross - tax
}
```

`let` names a value for the rest of the body.

## Calling a function

A call looks like any other expression and can go anywhere an expression can:
in a value, an entry or a check. Write the `(` right after the function's
name, as in `net(gross, 0.3)`; with a space between, it isn't a call.

```
let salary = 120_000
let gross = double(salary)

entry pay @ last {
  income.salary -> assets.cash : net(gross / 12, 0.3)
}
```

## Built-in functions

| Function | Returns |
|----------|---------|
| `min(a, b)`, `max(a, b)` | the smaller or larger |
| `abs(x)` | `x` without its sign |
| `floor(x)`, `ceil(x)`, `round(x)` | `x` rounded down, up, or to the nearest whole number (half to even) |
| `fill(x)` | what's left of a target (see [Entries](./entries.md#filling-a-target)) |

User-defined functions can call them:

```
fn positive(x) { max(x, 0) }
```

## Calling other functions

A function can call any other function, as long as no function ends up calling
itself.

```
fn double(x) { x * 2 }
fn quad(x)   { double(double(x)) }
```

## Conditional expressions

```
fn bonus(salary, target) {
  if target > 0 then salary * 0.1 else 0
}
```

## Values and balances

A function can use [values](./values.md), which have a value on every day. It
can't read account balances or totals, which change as entries fire; pass them
in as arguments instead.

```
let tax_rate = 28%

fn net(gross) { gross - gross * tax_rate }

fn after_rent(cash) { cash - 3_000 }

check after_rent(assets.cash) >= 0
```

Because values have the value for the current date, a function gives a
different result on different days when the values it uses change:

```
fn double(x) { x * 2 }

let salary {
  2025-01-01..2025-07-01 => 100
  2025-07-01..           => 200
}
let doubled = double(salary)
```

During January `doubled` is `200`; from July onwards it is `400`.

## Rates

Rates pass through functions: `salary` is per year, so `net(salary)` is too,
and an entry moves a twelfth of it each month (see [Rates](./entries.md#rates)).

```
let salary = 120_000/year

entry paycheck @ last {
  income.salary -> assets.cash : net(salary)
}
```
