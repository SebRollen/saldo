# Functions

User-defined functions let you name and reuse a computation across params,
entries, and assertions. They are pure — they take explicit arguments and
return a value; they cannot read global params or account balances.

## Syntax

```
fn <name>(<param>, ...) {
  [let <name> = <expression>;]
  ...
  [return] <expression>
}
```

The final expression is the return value. The `return` keyword is optional.

## Defining a function

```
fn double(x) { x * 2 }

fn net(gross, rate) {
  let tax = gross * rate;
  gross - tax
}
```

Local bindings introduced with `let` are available for the rest of the body.

## Calling a function

A function call looks like any other expression and can appear anywhere an
expression is valid: in a param definition, an entry amount, or an
assertion.

```
param gross = double(salary)

entry monthly "pay" {
  Assets:Cash = net(gross / 12, 0.3)
  Income:Salary
}
```

## Calling built-ins

User-defined functions can call built-in functions such as `min` and `max`.

```
fn positive(x) { max(x, 0) }
```

## Calling other functions

A function can call any other function defined in the file, as long as there
is no cycle.

```
fn double(x) { x * 2 }
fn quad(x)   { double(double(x)) }
```

## Conditional expressions

`if … then … else …` works inside function bodies.

```
fn bonus(salary, target) {
  return if target > 0 then salary * 0.1 else 0;
}
```

## Time-varying inputs

Functions have no special awareness of time, but because params are
evaluated as of the current simulation date before being passed in, a
function automatically produces a different result on different days when
its arguments are time-varying.

```
fn double(x) { x * 2 }

param salary {
  from 2025-01-01 to 2025-07-01 = 100
  from 2025-07-01               = 200
}
param doubled = double(salary)
```

During January `doubled` is `200`; from July onwards it is `400`. The
function definition itself never changes — only the value of `salary` at
the point it is called does.

## Restrictions

- **No global params.** Function bodies can only reference their own
  parameters and local `let` bindings. Referencing a global param or
  account inside a function body is an error.
- **No recursion.** A function cannot call itself, directly or through a
  cycle of calls.
- **No duplicate names.** Each function name must be unique within the file.
- **Arity is checked.** Calling a function with the wrong number of arguments
  is an error.

## Full example

```
fn net(gross, rate) {
  let tax = gross * rate;
  gross - tax
}

param salary : usd/year {
  from 2025-01-01 to 2026-01-01 = 120_000
  from 2026-01-01               = 140_000
}

entry monthly "paycheck" {
  Assets:Cash    = net(salary / 12, 0.28)
  Expenses:Tax   = salary / 12 * 0.28
  Income:Salary
}
```
