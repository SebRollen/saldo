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
        $.schedule_decl,
        $.entry_decl,
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
    // Schedule declaration
    //   schedule semi_monthly = every month on the 15th, last day
    //   schedule every_two = every second friday from 2026-01-02
    // -----------------------------------------------------------------------

    schedule_decl: ($) =>
      seq(
        "schedule",
        field("name", $.identifier),
        "=",
        field("schedule", $.schedule_literal),
      ),

    // -----------------------------------------------------------------------
    // Entry declaration
    //   entry monthly "Jim's paycheck" { ... }
    //   entry semi_monthly "Seb's paycheck" { ... } as seb_paycheck
    //   entry yearly on dec 11 "Annual bonus" { ... } as bonus
    // -----------------------------------------------------------------------

    entry_decl: ($) =>
      seq(
        "entry",
        field("schedule", $.schedule_ref),
        field("label", $.string),
        "{",
        repeat($.posting),
        "}",
        optional(seq("as", field("alias", $.identifier))),
      ),

    // -----------------------------------------------------------------------
    // Assert declaration
    //   assert Assets:Cash >= 0
    //   assert 2026-12-31 Assets:Retirement:Seb == 24_500
    //   assert yearly Assets:Cash >= 0
    // -----------------------------------------------------------------------

    assert_decl: ($) =>
      seq(
        "assert",
        optional(field("schedule", $.schedule_literal)),
        field("condition", $._expr),
      ),

    // -----------------------------------------------------------------------
    // Schedule reference — literal schedule or a named schedule identifier
    // -----------------------------------------------------------------------

    schedule_ref: ($) =>
      choice(
        prec(1, $.schedule_literal),
        $.identifier,
      ),

    // -----------------------------------------------------------------------
    // Schedule literal
    //   daily / weekly / monthly / quarterly / yearly / annually [on ...]
    //   every [nth] period [on ...] [from date]
    //   2026-01-01 [, 2026-02-01 ...]
    // -----------------------------------------------------------------------

    schedule_literal: ($) =>
      choice(
        $.adverbial_schedule,
        $.every_schedule,
        $.date_schedule,
      ),

    adverbial_schedule: ($) =>
      seq(
        field(
          "kind",
          choice("daily", "weekly", "monthly", "quarterly", "yearly", "annually"),
        ),
        optional($.on_clause),
      ),

    every_schedule: ($) =>
      seq(
        "every",
        optional(field("nth", $._ordinal)),
        field("period", $._period_spec),
        optional(seq("from", field("start", $.date))),
      ),

    _period_spec: ($) =>
      choice(
        choice("day", "days"),
        seq(choice("week", "weeks"), optional($.on_clause)),
        seq(choice("month", "months"), optional($.on_clause)),
        choice("quarter", "quarters"),
        seq(choice("year", "years"), optional($.on_clause)),
        $.day_of_week,
        // month_name uses _period_ordinal (no bare integer) to avoid ambiguity with
        // the expression that follows the schedule in assert_decl.
        seq($.month_name, optional($._period_ordinal)),
      ),

    // Ordinals in period specs: suffix (15th) or word (first/last/second/…) only.
    // Plain integers are excluded here to prevent ambiguity with following expressions.
    _period_ordinal: ($) =>
      choice(
        $.ordinal_suffix,
        $.ordinal_word,
      ),

    on_clause: ($) =>
      seq(
        "on",
        optional("the"),
        $._occurrence_list,
      ),

    _occurrence_list: ($) =>
      seq(
        $._occurrence_item,
        repeat(seq(choice(",", "and"), optional("the"), $._occurrence_item)),
      ),

    // An item in an on-clause can be:
    //   15th day / last monday / first  (ordinal with optional day/dow)
    //   dec 31 / jan first              (month name + ordinal)
    //   2026-01-15                      (explicit date)
    _occurrence_item: ($) =>
      choice(
        seq($._ordinal, optional(choice($.day_of_week, "day", "days"))),
        seq($.month_name, $._ordinal),
        $.date,
      ),

    // Ordinals may be: 1st/2nd/15th (suffix form), a plain integer (11, 31),
    // or a word-form (first, last, second, third, ...).
    _ordinal: ($) =>
      choice(
        $.ordinal_suffix,
        $.integer,
        $.ordinal_word,
      ),

    ordinal_word: (_) =>
      choice(
        "first", "last",
        "second", "third", "fourth", "fifth",
        "sixth", "seventh", "eighth", "ninth", "tenth",
      ),

    // 1st, 2nd, 3rd, 15th …
    ordinal_suffix: (_) => token(/[0-9]+(st|nd|rd|th)/),

    date_schedule: ($) =>
      seq(
        $.date,
        repeat(seq(choice(",", "and"), $.date)),
      ),

    // -----------------------------------------------------------------------
    // Posting  (inside entry body)
    //   Assets:Retirement:Seb = expr [as leg_name]
    //   Income:Gross:Salary:Jim        [as leg_name]
    // -----------------------------------------------------------------------

    posting: ($) =>
      prec.right(
        seq(
          field("account", $.colon_path),
          optional(seq("=", field("amount", $.posting_amount))),
          optional(seq("as", field("leg_name", $.identifier))),
        ),
      ),

    posting_amount: ($) => choice("all", $._expr),

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
    // Day-of-week names
    // -----------------------------------------------------------------------

    day_of_week: (_) =>
      choice(
        "monday", "mondays", "mon",
        "tuesday", "tuesdays", "tue",
        "wednesday", "wednesdays", "wed",
        "thursday", "thursdays", "thu",
        "friday", "fridays", "fri",
        "saturday", "saturdays", "sat",
        "sunday", "sundays", "sun",
        "weekday", "weekdays",
        "weekend",
      ),

    // -----------------------------------------------------------------------
    // Month names
    // -----------------------------------------------------------------------

    month_name: (_) =>
      choice(
        "january", "jan",
        "february", "feb",
        "march", "mar",
        "april", "apr",
        "may",
        "june", "jun",
        "july", "jul",
        "august", "aug",
        "september", "sep",
        "october", "oct",
        "november", "nov",
        "december", "dec",
      ),

    // -----------------------------------------------------------------------
    // Terminals
    // -----------------------------------------------------------------------

    identifier: (_) => /[a-zA-Z_][a-zA-Z0-9_]*/,

    date: (_) => token(/\d{4}-\d{2}-\d{2}/),

    float: (_) => token(/[0-9][0-9_]*\.[0-9][0-9_]*/),

    integer: (_) => token(/[0-9][0-9_]*/),

    boolean: (_) => choice("true", "false"),

    string: (_) => token(seq('"', /[^"]*/, '"')),

    comment: (_) => token(seq("//", /.*/)),
  },
});
