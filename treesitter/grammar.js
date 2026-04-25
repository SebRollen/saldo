/// <reference types="tree-sitter-cli/dsl" />
// @ts-check

module.exports = grammar({
  name: "saldo",

  word: ($) => $.identifier,

  extras: ($) => [/\s+/, $.comment],

  rules: {
    // -----------------------------------------------------------------------
    // Top level
    // -----------------------------------------------------------------------

    source_file: ($) => repeat($.declaration),

    declaration: ($) =>
      choice(
        $.account_decl,
        $.param_decl,
        $.flow_decl,
        $.assert_decl,
      ),

    // -----------------------------------------------------------------------
    // Account declaration
    //   account Assets:Cash = 5_000
    //   account Assets:Retirement:Jim
    // -----------------------------------------------------------------------

    account_decl: ($) =>
      seq(
        "account",
        field("name", $.colon_path),
        optional(seq("=", field("init", $._expr))),
      ),

    // -----------------------------------------------------------------------
    // Param declaration
    //   param interest_rate = 0.05
    //   param jim_salary : usd/year {
    //     from 2025-12-31 to 2026-04-01 = 115_000
    //   }
    // -----------------------------------------------------------------------

    param_decl: ($) =>
      seq(
        "param",
        field("name", $.identifier),
        optional(seq(":", field("unit", $.unit))),
        choice($.const_body, $.schedule_body),
      ),

    const_body: ($) => seq("=", $._expr),

    schedule_body: ($) =>
      seq(
        "{",
        repeat($.interval),
        "}",
      ),

    interval: ($) =>
      seq(
        "from",
        field("from", $.date),
        optional(seq("to", field("to", $.date))),
        "=",
        field("value", $._expr),
      ),

    // -----------------------------------------------------------------------
    // Flow declaration (indentation-based in source; layout pass injects braces)
    //   monthly jim_paycheck
    //     Assets:Retirement:Jim  min(...) as retirement_contribution
    //     Assets:Cash            jim_salary / 12 - retirement_contribution
    //     Income:Gross:Salary:Jim
    // -----------------------------------------------------------------------

    flow_decl: ($) =>
      seq(
        field("schedule", $.schedule_kind),
        field("name", $.identifier),
        repeat($.posting),
      ),

    schedule_kind: ($) =>
      choice(
        "daily",
        "monthly",
        "quarterly",
        "yearly",
        seq("on", "(", field("date", $.date), ")"),
      ),

    // A posting line: <colon_path> [<amount>] [as <ident>]
    // prec.right resolves the shift-reduce conflict between ending the posting
    // and continuing to parse the amount — prefer shift (consume more tokens).
    posting: ($) =>
      prec.right(
        seq(
          field("account", $.colon_path),
          optional(field("amount", $.posting_amount)),
          optional(seq("as", field("leg_name", $.identifier))),
        ),
      ),

    posting_amount: ($) => choice("all", $._expr),

    // -----------------------------------------------------------------------
    // Assert declaration
    //   assert Assets:Cash >= 0
    //   assert yearly Assets:Retirement:Jim <= 24_500
    //   assert on(2026-12-31) Assets:Retirement:Beth == 24_500
    // -----------------------------------------------------------------------

    assert_decl: ($) =>
      seq(
        "assert",
        optional(field("schedule", $.schedule_kind)),
        field("condition", $._expr),
      ),

    // -----------------------------------------------------------------------
    // Unit annotation  e.g. `usd`, `usd/year`, `%`
    // -----------------------------------------------------------------------

    unit: ($) => seq($._unit_atom, optional(seq("/", $._unit_atom))),

    _unit_atom: ($) => choice($.identifier, "%"),

    // -----------------------------------------------------------------------
    // Colon-separated path  e.g. `Assets:Cash`, `Income:Gross:Salary:Jim`
    // -----------------------------------------------------------------------

    colon_path: ($) =>
      seq(
        $.identifier,
        repeat(seq(":", $.identifier)),
      ),

    // -----------------------------------------------------------------------
    // Expressions
    // -----------------------------------------------------------------------

    _expr: ($) =>
      choice(
        $.if_expr,
        $.binary_expr,
        $.unary_minus,
        $.call_expr,
        $.agg_expr,
        $.colon_path,
        $.integer,
        $.float,
        $.boolean,
        $.parenthesized_expr,
      ),

    // if <cond> then <then> else <else>
    if_expr: ($) =>
      prec.right(
        0,
        seq(
          "if",
          field("condition", $._expr),
          "then",
          field("consequence", $._expr),
          "else",
          field("alternative", $._expr),
        ),
      ),

    // Binary operators, by ascending precedence
    binary_expr: ($) =>
      choice(
        prec.left(
          1,
          seq(
            field("left", $._expr),
            field("operator", $.comparison_operator),
            field("right", $._expr),
          ),
        ),
        prec.left(
          2,
          seq(
            field("left", $._expr),
            field("operator", $.additive_operator),
            field("right", $._expr),
          ),
        ),
        prec.left(
          3,
          seq(
            field("left", $._expr),
            field("operator", $.multiplicative_operator),
            field("right", $._expr),
          ),
        ),
      ),

    comparison_operator: (_) => choice("==", "<", ">", "<=", ">="),
    additive_operator: (_) => choice("+", "-"),
    multiplicative_operator: (_) => choice("*", "/"),

    unary_minus: ($) => prec(4, seq("-", field("operand", $._expr))),

    // Function call  e.g. `min(cash, 2_000)`
    call_expr: ($) =>
      prec(
        5,
        seq(
          field("function", $.identifier),
          "(",
          field(
            "arguments",
            optional(
              seq($._expr, repeat(seq(",", $._expr)), optional(",")),
            ),
          ),
          ")",
        ),
      ),

    // YTD/QTD/MTD aggregation access
    //   retirement_contribution.ytd          (unqualified)
    //   paycheck.retirement_contribution.ytd  (flow-qualified)
    agg_expr: ($) =>
      prec(
        6,
        choice(
          seq(
            field("flow", $.identifier),
            ".",
            field("leg", $.identifier),
            ".",
            field("kind", $.agg_kind),
          ),
          seq(
            field("leg", $.identifier),
            ".",
            field("kind", $.agg_kind),
          ),
        ),
      ),

    agg_kind: (_) => choice("ytd", "qtd", "mtd"),

    parenthesized_expr: ($) => seq("(", $._expr, ")"),

    // -----------------------------------------------------------------------
    // Terminals
    // -----------------------------------------------------------------------

    identifier: (_) => /[a-zA-Z_][a-zA-Z0-9_]*/,

    // Bare date (YYYY-MM-DD) or @-prefixed date (@YYYY-MM-DD).
    date: (_) =>
      token(
        choice(
          /@\d{4}-\d{2}-\d{2}/,
          /\d{4}-\d{2}-\d{2}/,
        ),
      ),

    // Float before integer so `0.05` isn't split into `0` + `.05`.
    float: (_) => token(/[0-9][0-9_]*\.[0-9][0-9_]*/),

    integer: (_) => token(/[0-9][0-9_]*/),

    boolean: (_) => choice("true", "false"),

    comment: (_) => token(seq("//", /.*/)),
  },
});
