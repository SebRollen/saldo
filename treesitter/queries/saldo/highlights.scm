; Declaration keywords
[
  "account"
  "param"
  "schedule"
  "entry"
  "assert"
  "that"
  "fn"
  "let"
] @keyword

"return" @keyword.return

; Temporal / interval keywords
[
  "from"
  "to"
] @keyword

; Schedule period keywords
[
  "every"
  "daily"
  "weekly"
  "monthly"
  "quarterly"
  "yearly"
  "annually"
  "on"
  "the"
] @keyword

; Day-of-week names
(day_of_week) @keyword

; Month names
(month_name) @keyword

; Ordinal words (first, last, second, …)
(ordinal_word) @keyword

; Posting / binding keyword
"as" @keyword

; Posting wildcard
"all" @keyword.builtin

; Aggregation kinds
(agg_kind) @keyword.builtin

; Boolean literals
(boolean) @boolean

; Control flow
[
  "if"
  "then"
  "else"
] @keyword.control

; Logical operators
[
  "and"
  "or"
  "not"
  "per"
] @keyword.operator

; Operators
[
  "="
  "==" "!="
  "<" ">" "<=" ">="
  "@"
  "+" "-" "*" "/" "%"
] @operator

; Literals
(integer)       @number
(float)         @number.float
(ordinal_suffix) @number
(date)          @string.special
(string)        @string

; Comments
(comment) @comment

; Declaration names
(account_decl  name: (colon_path)  @variable)
(param_decl    name: (identifier)  @variable)
(schedule_decl name: (identifier)  @function)
(entry_decl    alias: (identifier) @function)
(fn_decl       name: (identifier)  @function)
(fn_decl       parameter: (identifier) @variable.parameter)
(let_stmt      name: (identifier)  @variable)

; Entry label string highlighted as a function name (it names the entry)
(entry_decl label: (string) @function)

; Posting account path and optional leg label
(posting account:  (colon_path)  @variable)
(posting leg_name: (identifier)  @label)

; Aggregation expressions — flow/leg qualifiers
(agg_expr flow: (identifier) @variable)
(agg_expr leg:  (identifier) @variable)

; Named schedule reference in entry/assert
(schedule_ref (identifier) @function)

; Builtin function calls
(call_expr function: (identifier) @function.builtin)

; Rate units
(per_expr unit: (time_unit) @type)

; Brackets and delimiters
[ "{" "}" "(" ")" ] @punctuation.bracket
[ ":" "," "." ";" ] @punctuation.delimiter
