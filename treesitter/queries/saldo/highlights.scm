; Account declaration keyword
"account" @keyword.type

; Declaration keywords
[
  "param"
  "assert"
] @keyword

; Schedule kinds
[
  "daily"
  "monthly"
  "quarterly"
  "yearly"
  "on"
] @keyword

; Temporal / interval keywords
[
  "from"
  "to"
] @keyword

; Posting label keyword
"as" @keyword

; Posting wildcard
"all" @keyword.builtin

; Aggregation kinds
(agg_kind) @keyword.builtin

; Control flow
[
  "if"
  "then"
  "else"
] @keyword.control

; Operators
[
  "="
  "=="
  "<" ">" "<=" ">="
  "+" "-" "*" "/"
] @operator

; Literals
(integer) @number
(float)   @number.float
(date)    @string.special
(boolean) @boolean

; Comments
(comment) @comment

; Declaration names
(account_decl  name: (colon_path)  @variable)
(param_decl    name: (identifier)  @variable)
(flow_decl     name: (identifier)  @function)

; Posting account path and optional leg label
(posting account:  (colon_path)  @variable)
(posting leg_name: (identifier)  @label)

; Aggregation expressions — flow/leg qualifiers
(agg_expr flow: (identifier) @variable)
(agg_expr leg:  (identifier) @variable)

; Builtin function calls
(call_expr function: (identifier) @function.builtin)

; Unit annotations
(unit) @type

; Brackets and delimiters
[ "{" "}" "(" ")" ] @punctuation.bracket
[ ":" "," "."     ] @punctuation.delimiter
