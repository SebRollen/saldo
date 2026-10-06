/// <reference types="tree-sitter-cli/dsl" />
// @ts-check

// How tightly operators bind, loosest first, as in saldo's own parser.
const PREC = {
  or: 1,
  and: 2,
  not: 3,
  compare: 4,
  union: 5,
  inter: 6,
  range: 7,
  sum: 8,
  product: 9,
  unary: 10,
  power: 11,
  postfix: 12,
  call: 13,
};

const commaSep1 = (rule) => seq(rule, repeat(seq(",", rule)), optional(","));

module.exports = grammar({
  name: "saldo",

  word: ($) => $.identifier,

  extras: ($) => [/\s+/, $.comment],

  // Whether `a` in `let a.b.c` starts the module or names the value is only
  // known at the `=`.
  conflicts: ($) => [[$.module_path]],

  rules: {
    // -----------------------------------------------------------------------
    // Top level
    // -----------------------------------------------------------------------

    // A scenario's own file holds what its braces would, so `drop` can be at
    // the top too.
    source_file: ($) =>
      repeat(choice($._declaration, $.scenario_decl, $.drop_decl)),

    _declaration: ($) =>
      choice(
        $.let_decl,
        $.fn_decl,
        $.entry_decl,
        $.check_decl,
        $.balances_decl,
        $.mod_decl,
      ),

    // -----------------------------------------------------------------------
    // Values and sets of days
    //   let interest_rate = 5%/year
    //   let payday = 15th | last
    //   let daycare {
    //     jan 2027     => 0
    //     jul | aug    => 0
    //     2027-09-01.. => 2_100 compounding 3% every year
    //     _            => 1_950
    //   }
    // -----------------------------------------------------------------------

    let_decl: ($) =>
      seq(
        "let",
        optional(field("module", $.module_path)),
        field("name", $.identifier),
        choice(seq("=", field("value", $._expr)), field("arms", $.arms)),
      ),

    arms: ($) => seq("{", repeat($.arm), "}"),

    arm: ($) =>
      seq(
        field("days", choice($.wildcard, $._expr)),
        "=>",
        field("value", $._expr),
        optional(field("growth", $.growth)),
      ),

    wildcard: (_) => "_",

    // compounding 4% every year / adding 100/month every year
    growth: ($) =>
      seq(
        field("kind", choice("compounding", "adding")),
        field("by", $._expr),
        "every",
        field("every", $.time_unit),
      ),

    // -----------------------------------------------------------------------
    // Functions
    //   fn net(gross, rate) {
    //     let tax = gross * rate
    //     gross - tax
    //   }
    //   fn loan(debt, rate) { mod { … } }
    // -----------------------------------------------------------------------

    fn_decl: ($) =>
      seq(
        "fn",
        optional(field("module", $.module_path)),
        field("name", $.identifier),
        "(",
        optional(commaSep1(field("parameter", $.identifier))),
        ")",
        "{",
        repeat($.let_stmt),
        choice(field("value", $._expr), field("module", $.module_body)),
        "}",
      ),

    let_stmt: ($) =>
      seq("let", field("name", $.identifier), "=", field("value", $._expr)),

    module_body: ($) => seq("mod", "{", repeat($._declaration), "}"),

    // -----------------------------------------------------------------------
    // Entries
    //   entry paycheck "Paycheck" @ 15th | last {
    //     income.salary -> assets.cash : salary
    //     assets.cash -> assets.retirement : max_401k #contribution
    //   }
    // -----------------------------------------------------------------------

    entry_decl: ($) =>
      seq(
        "entry",
        optional(field("module", $.module_path)),
        field("name", $.identifier),
        optional(field("label", $.string)),
        optional(seq("@", field("days", $._expr))),
        optional(seq("after", commaSep1(field("after", $.path)))),
        field("body", $.block),
      ),

    block: ($) => seq("{", repeat($._entry_stmt), "}"),

    _entry_stmt: ($) => choice($.let_stmt, $.move, $.if_stmt, $.for_stmt),

    move: ($) =>
      prec.right(
        seq(
          field("from", $.path),
          "->",
          field("to", $.path),
          ":",
          field("amount", $._expr),
          optional(seq("#", field("tag", $.identifier))),
        ),
      ),

    if_stmt: ($) =>
      prec.right(
        seq(
          "if",
          field("condition", $._expr),
          field("consequence", $.block),
          optional(
            seq("else", field("alternative", choice($.if_stmt, $.block))),
          ),
        ),
      ),

    for_stmt: ($) =>
      seq(
        "for",
        field("variable", $.identifier),
        "in",
        commaSep1(field("accounts", $.path)),
        field("body", $.block),
      ),

    // -----------------------------------------------------------------------
    // Checks and balances
    //   check assets.cash >= 0
    //   check assets.retirement == 24_500 @ 2026-12-31
    //   balances @ 2026-01-01 { assets.cash = 5_000  pay.k401[year] = 1_000 }
    // -----------------------------------------------------------------------

    check_decl: ($) =>
      prec.right(
        seq(
          "check",
          field("condition", $._expr),
          optional(seq("@", field("days", $._expr))),
        ),
      ),

    balances_decl: ($) =>
      seq("balances", "@", field("date", $.date), "{", repeat($.balance), "}"),

    // `assets.cash = 5_000`, or a total so far: `pay.contribution[year] = 1_000`
    balance: ($) =>
      seq(
        field("account", $.path),
        optional(seq("[", field("unit", $.time_unit), "]")),
        "=",
        field("value", $._expr),
      ),

    // -----------------------------------------------------------------------
    // Modules
    //   mod loans
    //   mod pay { … }
    //   mod car = loan(debt: liabilities.loans.car, rate: 6%/year)
    // -----------------------------------------------------------------------

    mod_decl: ($) =>
      prec.right(
        seq(
          "mod",
          optional(field("module", $.module_path)),
          field("name", $.identifier),
          optional(
            choice(
              seq("{", repeat($._declaration), "}"),
              seq("=", field("call", $.call_expr)),
            ),
          ),
        ),
      ),

    // -----------------------------------------------------------------------
    // Scenarios
    //   scenario prepay {
    //     let loans.payment = 1_200
    //     drop childcare.nanny @ 2027-09-01..
    //   }
    //   scenario house
    // -----------------------------------------------------------------------

    scenario_decl: ($) =>
      prec.right(
        seq(
          "scenario",
          field("name", $.identifier),
          optional(seq("{", repeat(choice($._declaration, $.drop_decl)), "}")),
        ),
      ),

    drop_decl: ($) =>
      prec.right(
        seq(
          "drop",
          field("target", $.path),
          optional(seq("@", field("days", $._expr))),
        ),
      ),

    // `loans.` in `let loans.payment = 1_200`, which a scenario declares in
    // module `loans`.
    module_path: ($) => repeat1(seq($.identifier, ".")),

    // -----------------------------------------------------------------------
    // Names: `assets.cash`, `loans.rate`, `paycheck.contribution`
    // -----------------------------------------------------------------------

    path: ($) => prec.right(seq($.identifier, repeat(seq(".", $.identifier)))),

    // -----------------------------------------------------------------------
    // Expressions
    // -----------------------------------------------------------------------

    _expr: ($) =>
      choice(
        $.if_expr,
        $.binary_expr,
        $.not_expr,
        $.unary_minus,
        $.percent_expr,
        $.rate_expr,
        $.period_expr,
        $.range_expr,
        $.call_expr,
        $.path,
        $.integer,
        $.float,
        $.boolean,
        $.date,
        $._days,
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

    binary_expr: ($) => {
      const level = (precedence, operator) =>
        prec.left(
          precedence,
          seq(
            field("left", $._expr),
            field("operator", operator),
            field("right", $._expr),
          ),
        );
      return choice(
        level(PREC.or, $.or_operator),
        level(PREC.and, $.and_operator),
        level(PREC.compare, $.comparison_operator),
        level(PREC.union, $.union_operator),
        level(PREC.inter, $.inter_operator),
        level(PREC.sum, $.additive_operator),
        level(PREC.product, $.multiplicative_operator),
        // `2^3^2` is 2^9, and `-2^2` is -4.
        prec.right(
          PREC.power,
          seq(
            field("left", $._expr),
            field("operator", $.power_operator),
            field("right", $._expr),
          ),
        ),
      );
    },

    or_operator: (_) => "or",
    and_operator: (_) => "and",
    comparison_operator: (_) => choice("==", "!=", "<", ">", "<=", ">="),
    union_operator: (_) => "|",
    inter_operator: (_) => "&",
    additive_operator: (_) => choice("+", "-"),
    multiplicative_operator: (_) => choice("*", "/"),
    power_operator: (_) => "^",

    not_expr: ($) => prec(PREC.not, seq("not", field("operand", $._expr))),

    unary_minus: ($) => prec(PREC.unary, seq("-", field("operand", $._expr))),

    // `6.2%` is 0.062.
    percent_expr: ($) =>
      prec.left(PREC.postfix, seq(field("operand", $._expr), "%")),

    // `95_000/year`: an amount per period.
    rate_expr: ($) =>
      prec.left(
        PREC.product,
        seq(field("value", $._expr), "/", field("unit", $.time_unit)),
      ),

    // `24_500[year]` or `paycheck.contribution[year]`: a limit, or a total.
    period_expr: ($) =>
      prec.left(
        PREC.postfix,
        seq(field("value", $._expr), "[", field("unit", $.time_unit), "]"),
      ),

    // `2026-01-01..2027-01-01`, `sep..=jun`, `2026..`
    range_expr: ($) =>
      prec.left(
        PREC.range,
        seq(
          field("from", $._expr),
          choice("..", "..="),
          optional(field("to", $._expr)),
        ),
      ),

    time_unit: (_) => choice("day", "week", "month", "quarter", "year"),

    // A call's `(` comes right after the function's name.
    call_expr: ($) =>
      prec(
        PREC.call,
        seq(
          field("function", $.path),
          token.immediate("("),
          optional(commaSep1(field("argument", choice($.named_argument, $._expr)))),
          ")",
        ),
      ),

    named_argument: ($) =>
      seq(field("name", $.identifier), ":", field("value", $._expr)),

    parenthesized_expr: ($) => seq("(", $._expr, ")"),

    // -----------------------------------------------------------------------
    // Sets of days
    //   *  fri  weekday  15th  last  2nd mon  last fri
    //   jan  jan 1  feb last  jul 2026  q2  q3 2027  2026-04-15
    // -----------------------------------------------------------------------

    _days: ($) =>
      choice($.every_day, $.weekday_days, $.month_day, $.month_days, $.quarter_days),

    every_day: (_) => "*",

    weekday_days: ($) => $.day_of_week,

    month_day: ($) =>
      prec.right(
        seq(
          field("day", choice($.ordinal, "last")),
          optional(field("weekday", $.day_of_week)),
        ),
      ),

    month_days: ($) =>
      prec.right(
        seq(
          field("month", $.month_name),
          optional(field("day", choice($.integer, $.ordinal, "last"))),
        ),
      ),

    quarter_days: ($) =>
      prec.right(seq(field("quarter", $.quarter), optional(field("year", $.integer)))),

    quarter: (_) => choice("q1", "q2", "q3", "q4"),

    // 1st, 2nd, 3rd, 15th …
    ordinal: (_) => token(/[0-9]+(st|nd|rd|th)/),

    day_of_week: (_) =>
      choice(
        "monday", "mon",
        "tuesday", "tue",
        "wednesday", "wed",
        "thursday", "thu",
        "friday", "fri",
        "saturday", "sat",
        "sunday", "sun",
        "weekday",
        "weekend",
      ),

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

    // Letters and digits from any script.
    identifier: (_) => /[\p{L}_][\p{L}\p{N}_]*/,

    date: (_) => token(/\d{4}-\d{2}-\d{2}/),

    float: (_) => token(/[0-9][0-9_]*\.[0-9][0-9_]*/),

    integer: (_) => token(/[0-9][0-9_]*/),

    boolean: (_) => choice("true", "false"),

    string: (_) => token(seq('"', /[^"\n]*/, '"')),

    comment: (_) => token(seq("//", /.*/)),
  },
});
