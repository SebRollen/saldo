; Declaration keywords
[
  "let"
  "fn"
  "entry"
  "check"
  "balances"
  "mod"
] @keyword

; Entry statements and modifiers
[
  "after"
  "compounding"
  "adding"
  "every"
] @keyword

[
  "if"
  "then"
  "else"
] @keyword.control.conditional

[
  "for"
  "in"
] @keyword.control.repeat

; Logical operators
[
  (and_operator)
  (or_operator)
  "not"
] @keyword.operator

; Sets of days
(day_of_week) @constant
(month_name) @constant
(quarter) @constant
(every_day) @constant
(month_day "last" @constant)
(month_days "last" @constant)
(ordinal) @number
(wildcard) @constant.builtin

; Units of time
(time_unit) @type

; Literals
(boolean) @boolean
(integer) @number
(float) @number.float
(date) @string.special
(string) @string

; Comments
(comment) @comment

; Accounts start with their kind
((path . (identifier) @type.builtin)
  (#any-of? @type.builtin "assets" "liabilities" "income" "expenses" "equity"))

; Declaration names
(let_decl name: (identifier) @variable)
(let_stmt name: (identifier) @variable)
(fn_decl name: (identifier) @function)
(fn_decl parameter: (identifier) @variable.parameter)
(entry_decl name: (identifier) @function)
(entry_decl label: (string) @string.special.symbol)
(mod_decl name: (identifier) @module)
(for_stmt variable: (identifier) @variable)
(named_argument name: (identifier) @variable.parameter)

; Tags
(move "#" @punctuation.special tag: (identifier) @label)

; Calls
(call_expr function: (path (identifier) @function.call .))

; Operators
[
  "=" "=>" "->"
  "==" "!=" "<" ">" "<=" ">="
  "+" "-" "*" "/" "%"
  (union_operator) (inter_operator)
  ".." "..="
  "@"
] @operator

; Brackets and delimiters
[ "{" "}" "(" ")" "[" "]" ] @punctuation.bracket
[ ":" "," "." ] @punctuation.delimiter
