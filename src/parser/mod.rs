mod days;

use crate::Span;
use crate::ast::{
    Arm, BinOp, Decl, EntryStmt, Expr, Growth, GrowthKind, LetBody, ModBody, Move, Path, Program,
    Spanned, SpannedExpr, Stmt, TimeUnit,
};
use crate::errors::Diagnostic;
use crate::lexer::Token;
use chrono::NaiveDate;
use rust_decimal::Decimal;

/// Upper bound on expression nesting (parentheses, unary minus, chained
/// operators), so pathological input can't overflow the stack while parsing or
/// evaluating.
const MAX_EXPR_DEPTH: usize = 256;

// How tightly binary operators bind, loosest first.
const OR: u8 = 1;
const AND: u8 = 2;
const NOT: u8 = 3;
const COMPARE: u8 = 4;
const UNION: u8 = 5;
const INTER: u8 = 6;
const RANGE: u8 = 7;
const SUM: u8 = 8;
const PRODUCT: u8 = 9;

#[derive(Clone, Copy)]
enum Infix {
    Op(BinOp),
    Range { inclusive: bool },
}

/// Words that end an expression rather than continue it as a name.
const STOP_WORDS: &[&str] = &[
    "and",
    "or",
    "not",
    "then",
    "else",
    "compounding",
    "adding",
    "every",
];

pub struct Parser<'src> {
    tokens: Vec<(Token<'src>, Span)>,
    current: usize,
    errors: Vec<Diagnostic>,
    last_span: Span,
    /// Current expression nesting depth, bounded by `MAX_EXPR_DEPTH`.
    depth: usize,
}

impl<'src> Parser<'src> {
    pub fn new(tokens: Vec<(Token<'src>, Span)>) -> Self {
        let last_span = tokens.first().map(|(_, s)| *s).unwrap_or(Span::new(0, 0));
        Self {
            tokens,
            current: 0,
            errors: Vec::new(),
            last_span,
            depth: 0,
        }
    }

    pub fn parse(&mut self) -> Result<Program, Vec<Diagnostic>> {
        let decls = self.parse_decls(false);
        if self.errors.is_empty() {
            Ok(Program { decls })
        } else {
            Err(std::mem::take(&mut self.errors))
        }
    }

    fn peek(&self) -> &Token<'src> {
        self.tokens
            .get(self.current)
            .map(|(t, _)| t)
            .unwrap_or(&Token::Eof)
    }

    fn peek_next(&self) -> &Token<'src> {
        self.tokens
            .get(self.current + 1)
            .map(|(t, _)| t)
            .unwrap_or(&Token::Eof)
    }

    fn peek_span(&self) -> Span {
        self.tokens
            .get(self.current)
            .map(|(_, s)| *s)
            .unwrap_or(self.last_span)
    }

    fn advance(&mut self) -> (Token<'src>, Span) {
        if self.current < self.tokens.len() {
            let (t, s) = self.tokens[self.current].clone();
            self.last_span = s;
            self.current += 1;
            (t, s)
        } else {
            (Token::Eof, self.last_span)
        }
    }

    fn eat(&mut self, expected: &Token) -> Option<Span> {
        if self.peek() == expected {
            let (_, s) = self.advance();
            Some(s)
        } else {
            None
        }
    }

    fn expect(&mut self, expected: &Token<'src>) -> Option<Span> {
        if let Some(s) = self.eat(expected) {
            return Some(s);
        }
        self.error_here(format!("expected `{expected}`"));
        None
    }

    /// Reports `message` at the next token. A `(` there that isn't right
    /// after a name was probably meant as a call.
    fn error_here(&mut self, message: impl Into<String>) {
        let span = self.peek_span();
        let mut diag = Diagnostic::new(span, message);
        if *self.peek() == Token::LParen && self.last_span.end < span.start {
            diag = diag.with_note(
                span,
                "to call a function, write `(` right after its name, like `min(a, b)`",
            );
        }
        self.errors.push(diag);
    }

    fn eat_ident(&mut self) -> Option<(&'src str, Span)> {
        if let Token::Ident(s) = self.peek() {
            let s = *s;
            let (_, span) = self.advance();
            Some((s, span))
        } else {
            None
        }
    }

    fn expect_ident(&mut self, what: &str) -> Option<Spanned<String>> {
        if let Some((name, span)) = self.eat_ident() {
            return Some((name.to_string(), span));
        }
        self.error_here(format!("expected {what}"));
        None
    }

    /// Consumes `word`, a contextual keyword lexed as an identifier.
    /// Keywords are lowercase only.
    fn eat_keyword(&mut self, word: &str) -> Option<Span> {
        if let Token::Ident(s) = self.peek()
            && *s == word
        {
            let (_, span) = self.advance();
            return Some(span);
        }
        None
    }

    // ---- declarations ----

    /// Declarations up to the end of the file, or to a `}` if `nested`.
    fn parse_decls(&mut self, nested: bool) -> Vec<Spanned<Decl>> {
        let mut decls = Vec::new();
        loop {
            match self.peek() {
                Token::Eof => break,
                Token::RBrace if nested => break,
                _ => {}
            }
            let start = self.peek_span();
            let err_count = self.errors.len();
            if let Some(decl) = self.parse_decl() {
                let span = Span::new(start.start, self.last_span.end);
                decls.push((decl, span));
            } else {
                // Every failure path should report something, but never let a
                // malformed declaration disappear without an error.
                if self.errors.len() == err_count {
                    let span = Span::new(start.start, self.last_span.end.max(start.end));
                    self.errors
                        .push(Diagnostic::new(span, "malformed declaration"));
                }
                // Keep only the first error from this failed declaration, then
                // skip to the next declaration to avoid cascades.
                self.errors.truncate(err_count + 1);
                self.synchronize();
            }
        }
        decls
    }

    /// Skips to something that can only start a declaration.
    fn synchronize(&mut self) {
        // Always move on, so a declaration that fails on its keyword doesn't
        // fail forever.
        self.advance();
        while !matches!(
            self.peek(),
            Token::Eof
                | Token::Let
                | Token::Fn
                | Token::Entry
                | Token::Check
                | Token::Balances
                | Token::Mod
        ) {
            self.advance();
        }
    }

    fn parse_decl(&mut self) -> Option<Decl> {
        match self.peek() {
            Token::Let => {
                self.advance();
                self.parse_let_decl()
            }
            Token::Fn => {
                self.advance();
                self.parse_fn_decl()
            }
            Token::Entry => {
                self.advance();
                self.parse_entry_decl()
            }
            Token::Check => {
                self.advance();
                self.parse_check_decl()
            }
            Token::Balances => {
                self.advance();
                self.parse_balances_decl()
            }
            Token::Mod => {
                self.advance();
                self.parse_mod_decl()
            }
            _ => {
                self.error_here(
                    "expected a declaration: `let`, `fn`, `entry`, `check`, `balances` or `mod`",
                );
                None
            }
        }
    }

    fn parse_let_decl(&mut self) -> Option<Decl> {
        let name = self.expect_ident("a name after `let`")?;
        let body = if self.eat(&Token::Eq).is_some() {
            LetBody::Expr(self.parse_expr()?)
        } else if self.eat(&Token::LBrace).is_some() {
            let mut arms = Vec::new();
            while !matches!(self.peek(), Token::RBrace | Token::Eof) {
                arms.push(self.parse_arm()?);
            }
            self.expect(&Token::RBrace)?;
            LetBody::Arms(arms)
        } else {
            self.error_here(format!("expected `=` or `{{` after `let {}`", name.0));
            return None;
        };
        Some(Decl::Let { name, body })
    }

    fn parse_arm(&mut self) -> Option<Arm> {
        let start = self.peek_span();
        let when = if self.eat_keyword("_").is_some() {
            None
        } else {
            Some(self.parse_expr()?)
        };
        if self.eat(&Token::FatArrow).is_none() {
            self.error_here("expected `=>` between an arm's days and its value");
            return None;
        }
        let value = self.parse_expr()?;
        let growth = self.parse_growth()?;
        let span = Span::new(start.start, self.last_span.end);
        Some(Arm {
            when,
            value,
            growth,
            span,
        })
    }

    /// `compounding <rate> every <unit>` or `adding <amount> every <unit>`
    /// after an arm's value: `Some(None)` if there's neither, and `None` if
    /// there's an error.
    fn parse_growth(&mut self) -> Option<Option<Growth>> {
        let start = self.peek_span();
        let kind = if self.eat_keyword("compounding").is_some() {
            GrowthKind::Compounding
        } else if self.eat_keyword("adding").is_some() {
            GrowthKind::Adding
        } else {
            return Some(None);
        };
        let by = self.parse_expr()?;
        if self.eat_keyword("every").is_none() {
            self.error_here(format!(
                "expected `every` and how often to step after `{}`, like `every year`",
                kind.keyword()
            ));
            return None;
        }
        let every = self.parse_unit("`every`")?;
        let span = Span::new(start.start, self.last_span.end);
        Some(Some(Growth {
            kind,
            by,
            every,
            span,
        }))
    }

    /// A unit of time, which `after` comes before.
    fn parse_unit(&mut self, after: &str) -> Option<TimeUnit> {
        if let Token::Ident(name) = self.peek()
            && let Some(unit) = TimeUnit::from_name(name)
        {
            self.advance();
            return Some(unit);
        }
        self.error_here(format!(
            "expected `day`, `week`, `month`, `quarter` or `year` after {after}"
        ));
        None
    }

    fn parse_fn_decl(&mut self) -> Option<Decl> {
        let name = self.expect_ident("a function name")?;
        self.expect(&Token::LParen)?;
        let mut params = Vec::new();
        while !matches!(self.peek(), Token::RParen | Token::Eof) {
            params.push(self.expect_ident("a parameter name")?);
            if self.eat(&Token::Comma).is_none() {
                break;
            }
        }
        self.expect(&Token::RParen)?;
        self.expect(&Token::LBrace)?;
        let mut body = Vec::new();
        while self.eat(&Token::Let).is_some() {
            let (local, _) = self.expect_ident("a name after `let`")?;
            self.expect(&Token::Eq)?;
            let value = self.parse_expr()?;
            body.push(Stmt::Let { name: local, value });
        }
        if *self.peek() == Token::RBrace {
            self.error_here(format!(
                "function `{}` needs a value to return: end it with an expression",
                name.0
            ));
            return None;
        }
        if *self.peek() == Token::Mod && *self.peek_next() == Token::LBrace {
            self.advance();
            self.advance();
            let decls = self.parse_decls(true);
            self.expect(&Token::RBrace)?;
            body.push(Stmt::Mod(decls));
        } else {
            body.push(Stmt::Return(self.parse_expr()?));
        }
        if *self.peek() != Token::RBrace {
            self.error_here("expected `}` after the function's value");
            return None;
        }
        self.advance();
        Some(Decl::Fn { name, params, body })
    }

    fn parse_entry_decl(&mut self) -> Option<Decl> {
        let name = self.expect_ident("an entry name after `entry`")?;
        let label = if let Token::Str(s) = self.peek() {
            let s = s.to_string();
            self.advance();
            Some(s)
        } else {
            None
        };
        let when = if self.eat(&Token::At).is_some() {
            Some(self.parse_expr()?)
        } else {
            None
        };
        let mut after = Vec::new();
        if self.eat_keyword("after").is_some() {
            loop {
                let Some(entry) = self.try_parse_path() else {
                    self.error_here("expected an entry after `after`");
                    return None;
                };
                after.push(entry);
                if self.eat(&Token::Comma).is_none() {
                    break;
                }
            }
        }
        if *self.peek() != Token::LBrace {
            self.error_here("expected `{` to start the entry, or `@` and when it fires");
            return None;
        }
        self.advance();
        let body = self.parse_entry_block()?;
        self.expect(&Token::RBrace)?;
        Some(Decl::Entry {
            name,
            label,
            when,
            after,
            body,
        })
    }

    /// Statements up to the `}` that ends a block, which is left to eat.
    fn parse_entry_block(&mut self) -> Option<Vec<EntryStmt>> {
        let mut body = Vec::new();
        while !matches!(self.peek(), Token::RBrace | Token::Eof) {
            body.push(self.parse_entry_stmt()?);
        }
        Some(body)
    }

    /// `if c { … } else if d { … } else { … }`, after the `if`.
    fn parse_if_stmt(&mut self) -> Option<EntryStmt> {
        let mut branches = Vec::new();
        let mut otherwise = Vec::new();
        loop {
            let cond = self.parse_expr()?;
            self.expect(&Token::LBrace)?;
            let body = self.parse_entry_block()?;
            self.expect(&Token::RBrace)?;
            branches.push((cond, body));
            if self.eat_keyword("else").is_none() {
                break;
            }
            if self.eat_keyword("if").is_some() {
                continue;
            }
            self.expect(&Token::LBrace)?;
            otherwise = self.parse_entry_block()?;
            self.expect(&Token::RBrace)?;
            break;
        }
        Some(EntryStmt::If {
            branches,
            otherwise,
        })
    }

    fn parse_entry_stmt(&mut self) -> Option<EntryStmt> {
        if self.eat_keyword("if").is_some() {
            return self.parse_if_stmt();
        }
        if self.eat_keyword("for").is_some() {
            let var = self.expect_ident("a name after `for`")?;
            if self.eat_keyword("in").is_none() {
                self.error_here("expected `in` and the accounts to go through");
                return None;
            }
            let mut over = Vec::new();
            loop {
                let Some(path) = self.try_parse_path() else {
                    self.error_here("expected an account or a group of accounts");
                    return None;
                };
                over.push(path);
                if self.eat(&Token::Comma).is_none() {
                    break;
                }
            }
            self.expect(&Token::LBrace)?;
            let body = self.parse_entry_block()?;
            self.expect(&Token::RBrace)?;
            return Some(EntryStmt::For { var, over, body });
        }
        if self.eat(&Token::Let).is_some() {
            let name = self.expect_ident("a name after `let`")?;
            self.expect(&Token::Eq)?;
            let value = self.parse_expr()?;
            return Some(EntryStmt::Let { name, value });
        }
        let start = self.peek_span();
        let Some(from) = self.try_parse_path() else {
            self.error_here(
                "expected a move, like `assets.cash -> expenses.rent : 1_000`, or `let`",
            );
            return None;
        };
        if self.eat(&Token::Arrow).is_none() {
            self.error_here("expected `->` and the account the money moves to");
            return None;
        }
        let Some(to) = self.try_parse_path() else {
            self.error_here("expected the account the money moves to");
            return None;
        };
        if self.eat(&Token::Colon).is_none() {
            self.error_here("expected `:` and how much moves");
            return None;
        }
        let amount = self.parse_expr()?;
        let tag = if self.eat(&Token::Hash).is_some() {
            Some(self.expect_ident("a tag after `#`")?)
        } else {
            None
        };
        Some(EntryStmt::Move(Move {
            from,
            to,
            amount,
            tag,
            span: Span::new(start.start, self.last_span.end),
        }))
    }

    fn parse_check_decl(&mut self) -> Option<Decl> {
        let expr = self.parse_expr()?;
        let when = if self.eat(&Token::At).is_some() {
            Some(self.parse_expr()?)
        } else {
            None
        };
        Some(Decl::Check { expr, when })
    }

    fn parse_balances_decl(&mut self) -> Option<Decl> {
        if self.eat(&Token::At).is_none() {
            self.error_here("expected `@` and the date of the balances");
            return None;
        }
        let date = self.parse_date()?;
        self.expect(&Token::LBrace)?;
        let mut balances = Vec::new();
        while !matches!(self.peek(), Token::RBrace | Token::Eof) {
            let Some(path) = self.try_parse_path() else {
                self.error_here("expected an account, like `assets.cash = 1_000`");
                return None;
            };
            self.expect(&Token::Eq)?;
            balances.push((path, self.parse_expr()?));
        }
        self.expect(&Token::RBrace)?;
        Some(Decl::Balances { date, balances })
    }

    fn parse_mod_decl(&mut self) -> Option<Decl> {
        let name = self.expect_ident("a module name after `mod`")?;
        let body = if self.eat(&Token::LBrace).is_some() {
            let decls = self.parse_decls(true);
            self.expect(&Token::RBrace)?;
            ModBody::Inline(decls)
        } else if self.eat(&Token::Eq).is_some() {
            let call = self.parse_expr()?;
            if !matches!(call.0.as_ref(), Expr::Call(..)) {
                self.errors.push(Diagnostic::new(
                    call.1,
                    "expected a call to a function that makes a module, like `loan(…)`",
                ));
                return None;
            }
            ModBody::Call(call)
        } else {
            ModBody::File
        };
        Some(Decl::Mod { name, body })
    }

    pub(super) fn parse_date(&mut self) -> Option<NaiveDate> {
        if let Token::Date(d) = self.peek() {
            let d = *d;
            self.advance();
            Some(d)
        } else {
            self.error_here("expected a date, like `2026-01-01`");
            None
        }
    }

    /// A dotted name, like `assets.cash`, if one comes next.
    fn try_parse_path(&mut self) -> Option<Spanned<Path>> {
        let (first, start) = self.eat_ident()?;
        let mut parts = vec![first.to_string()];
        while *self.peek() == Token::Period {
            self.advance();
            match self.eat_ident() {
                Some((part, _)) => parts.push(part.to_string()),
                None => {
                    self.error_here("expected a name after `.`");
                    break;
                }
            }
        }
        Some((Path(parts), Span::new(start.start, self.last_span.end)))
    }

    // ---- expressions ----

    fn parse_expr(&mut self) -> Option<SpannedExpr> {
        let depth = self.depth;
        let expr = self.nest().and_then(|()| self.parse_binary(0));
        self.depth = depth;
        expr
    }

    /// Records one more level of expression nesting. `parse_expr` restores the
    /// depth when it returns.
    fn nest(&mut self) -> Option<()> {
        self.depth += 1;
        if self.depth > MAX_EXPR_DEPTH {
            self.error_here("expression is nested too deeply");
            return None;
        }
        Some(())
    }

    fn bin(left: SpannedExpr, op: BinOp, right: SpannedExpr) -> SpannedExpr {
        let span = Span::new(left.1.start, right.1.end);
        (Box::new(Expr::Bin(left, op, right)), span)
    }

    /// The binary operator that comes next, if any, and how tightly it binds.
    fn binary_op(&self) -> Option<(Infix, u8)> {
        use Infix::*;
        Some(match self.peek() {
            Token::Ident("or") => (Op(BinOp::Or), OR),
            Token::Ident("and") => (Op(BinOp::And), AND),
            Token::LtEq => (Op(BinOp::LtEq), COMPARE),
            Token::GtEq => (Op(BinOp::GtEq), COMPARE),
            Token::EqEq => (Op(BinOp::Eq), COMPARE),
            Token::Lt => (Op(BinOp::Lt), COMPARE),
            Token::Gt => (Op(BinOp::Gt), COMPARE),
            Token::NotEq => (Op(BinOp::NotEq), COMPARE),
            Token::Pipe => (Op(BinOp::Union), UNION),
            Token::Amp => (Op(BinOp::Inter), INTER),
            Token::DotDot => (Range { inclusive: false }, RANGE),
            Token::DotDotEq => (Range { inclusive: true }, RANGE),
            Token::Plus => (Op(BinOp::Add), SUM),
            Token::Minus => (Op(BinOp::Sub), SUM),
            Token::Star => (Op(BinOp::Mul), PRODUCT),
            Token::Slash => (Op(BinOp::Div), PRODUCT),
            _ => return None,
        })
    }

    /// An expression whose binary operators bind at least as tightly as
    /// `min`, by precedence climbing: from loosest to tightest, `or`, `and`,
    /// `not`, comparisons, `|`, `&`, ranges, `+ -`, and `* /`.
    fn parse_binary(&mut self, min: u8) -> Option<SpannedExpr> {
        let mut left = match self.eat_keyword("not") {
            Some(not_span) if min <= NOT => {
                self.nest()?;
                let inner = self.parse_binary(COMPARE)?;
                let span = Span::new(not_span.start, inner.1.end);
                (Box::new(Expr::Not(inner)), span)
            }
            Some(_) => {
                // `not` binds looser than this operator's operands.
                self.current -= 1;
                self.error_here("expected an expression; put `not …` in parentheses here");
                return None;
            }
            None => self.parse_unary()?,
        };
        while let Some((op, prec)) = self.binary_op() {
            if prec < min {
                break;
            }
            self.advance();
            self.nest()?;
            left = match op {
                Infix::Op(BinOp::Div)
                    if let Token::Ident(name) = self.peek()
                        && let Some(unit) = TimeUnit::from_name(name) =>
                {
                    // `95_000/year`: a rate.
                    self.advance();
                    let span = Span::new(left.1.start, self.last_span.end);
                    (Box::new(Expr::Per(left, unit)), span)
                }
                Infix::Op(op) => {
                    let right = self.parse_binary(prec + 1)?;
                    Self::bin(left, op, right)
                }
                Infix::Range { inclusive } => {
                    let to = if inclusive || self.starts_operand() {
                        Some(self.parse_binary(prec + 1)?)
                    } else {
                        None
                    };
                    let span = Span::new(left.1.start, self.last_span.end);
                    (
                        Box::new(Expr::Range {
                            from: left,
                            to,
                            inclusive,
                        }),
                        span,
                    )
                }
            };
            // Comparisons and ranges don't chain.
            if let Some((next, next_prec)) = self.binary_op()
                && next_prec == prec
                && (prec == COMPARE || matches!(next, Infix::Range { .. }))
            {
                let message = if prec == COMPARE {
                    "comparisons can't be chained; combine them with `and`"
                } else {
                    "ranges can't be chained"
                };
                self.errors.push(Diagnostic::new(
                    Span::new(left.1.start, self.peek_span().end),
                    message,
                ));
                return None;
            }
        }
        Some(left)
    }

    /// Whether the next token can start an operand, so an open range like
    /// `2026..` doesn't swallow what follows it.
    fn starts_operand(&self) -> bool {
        match self.peek() {
            Token::Number(_)
            | Token::Date(_)
            | Token::Ordinal(_)
            | Token::LParen
            | Token::Minus
            | Token::True
            | Token::False => true,
            Token::Ident(word) => !STOP_WORDS.contains(word) && *word != "_",
            _ => false,
        }
    }

    fn parse_unary(&mut self) -> Option<SpannedExpr> {
        if let Some(minus_span) = self.eat(&Token::Minus) {
            self.nest()?;
            let inner = self.parse_unary()?;
            let span = Span::new(minus_span.start, inner.1.end);
            Some((Box::new(Expr::Neg(inner)), span))
        } else {
            self.parse_postfix()
        }
    }

    /// An atom followed by any number of `%` and `[unit]`.
    fn parse_postfix(&mut self) -> Option<SpannedExpr> {
        let mut expr = self.parse_atom()?;
        loop {
            if let Some(percent) = self.eat(&Token::Percent) {
                self.nest()?;
                let span = Span::new(expr.1.start, percent.end);
                let hundred = Decimal::ONE_HUNDRED;
                let value = match *expr.0 {
                    Expr::Num(n) => Expr::Num(n / hundred),
                    _ => Expr::Bin(expr, BinOp::Div, (Box::new(Expr::Num(hundred)), percent)),
                };
                expr = (Box::new(value), span);
            } else if self.eat(&Token::LBracket).is_some() {
                self.nest()?;
                let unit = self.parse_unit("`[`")?;
                self.expect(&Token::RBracket)?;
                let span = Span::new(expr.1.start, self.last_span.end);
                expr = (Box::new(Expr::Period(expr, unit)), span);
            } else {
                return Some(expr);
            }
        }
    }

    fn parse_atom(&mut self) -> Option<SpannedExpr> {
        let start = self.peek_span();

        if self.eat_keyword("if").is_some() {
            return self.parse_if(start);
        }

        if let Some(s) = self.eat(&Token::True) {
            return Some((Box::new(Expr::Bool(true)), s));
        }
        if let Some(s) = self.eat(&Token::False) {
            return Some((Box::new(Expr::Bool(false)), s));
        }

        if let Token::Number(n) = self.peek() {
            let n = *n;
            let (_, s) = self.advance();
            return Some((Box::new(Expr::Num(n)), s));
        }

        if let Some(lit) = self.try_parse_days() {
            let span = Span::new(start.start, self.last_span.end);
            return Some((Box::new(Expr::Days(lit?)), span));
        }

        if matches!(self.peek(), Token::Ident(word) if !STOP_WORDS.contains(word)) {
            let (path, span) = self.try_parse_path()?;
            // A call's `(` comes right after the name, so `x (y)` on two
            // lines is two things rather than a call.
            if *self.peek() == Token::LParen && self.peek_span().start == span.end {
                return self.parse_call(path, span);
            }
            return Some((Box::new(Expr::Ref(path)), span));
        }

        if self.eat(&Token::LParen).is_some() {
            let inner = self.parse_expr()?;
            self.expect(&Token::RParen);
            return Some(inner);
        }

        self.error_here("expected an expression");
        None
    }

    /// `if … then … else …`, after the `if`.
    fn parse_if(&mut self, start: Span) -> Option<SpannedExpr> {
        let cond = self.parse_expr()?;
        if self.eat_keyword("then").is_none() {
            self.error_here("expected `then`");
            return None;
        }
        let then = self.parse_expr()?;
        if self.eat_keyword("else").is_none() {
            self.error_here("expected `else`");
            return None;
        }
        let else_ = self.parse_expr()?;
        let span = Span::new(start.start, self.last_span.end);
        Some((Box::new(Expr::If { cond, then, else_ }), span))
    }

    fn parse_call(&mut self, callee: Path, start: Span) -> Option<SpannedExpr> {
        self.expect(&Token::LParen);
        let mut args = Vec::new();
        while !matches!(self.peek(), Token::RParen | Token::Eof) {
            if let Token::Ident(name) = self.peek()
                && *self.peek_next() == Token::Colon
            {
                let name = (name.to_string(), self.peek_span());
                self.advance();
                self.advance();
                let value = self.parse_expr()?;
                let span = Span::new(name.1.start, value.1.end);
                args.push((Box::new(Expr::Named(name, value)), span));
            } else {
                args.push(self.parse_expr()?);
            }
            if self.eat(&Token::Comma).is_none() {
                break;
            }
        }
        self.expect(&Token::RParen)?;
        let span = Span::new(start.start, self.last_span.end);
        Some((Box::new(Expr::Call(callee, args)), span))
    }
}

pub fn parse(tokens: Vec<(Token<'_>, Span)>) -> Result<Program, Vec<Diagnostic>> {
    Parser::new(tokens).parse()
}

/// Parses `src` as a single expression.
#[cfg(test)]
pub fn parse_expression(src: &str) -> Result<SpannedExpr, Vec<Diagnostic>> {
    let tokens = crate::lexer::lex(src, 0)?;
    let mut parser = Parser::new(tokens);
    let expr = parser.parse_expr();
    if *parser.peek() != Token::Eof {
        parser.error_here("expected the end of the expression");
    }
    match expr {
        Some(expr) if parser.errors.is_empty() => Ok(expr),
        _ => Err(parser.errors),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::{DayLit, Dow, Month, Ordinal};
    use DayLit::*;

    fn parse_prog(src: &str) -> Program {
        let tokens = crate::lexer::lex(src, 0).expect("lexer errored");
        parse(tokens).unwrap_or_else(|errs| panic!("parse errs: {errs:?}"))
    }

    fn parse_errs(src: &str) -> Vec<Diagnostic> {
        let tokens = crate::lexer::lex(src, 0).expect("lexer errored");
        parse(tokens).unwrap_err()
    }

    fn expr(src: &str) -> Expr {
        *parse_expression(src)
            .unwrap_or_else(|errs| panic!("{src}: {errs:?}"))
            .0
    }

    #[test]
    fn parses_lets() {
        let prog = parse_prog(
            "let rate = 5%
             let salary {
               2026   => 95_000/year
               2027.. => 100_000/year compounding 3% every year
               _      => 0
             }",
        );
        let Decl::Let {
            name,
            body: LetBody::Expr(_),
        } = &prog.decls[0].0
        else {
            panic!("not a let")
        };
        assert_eq!(name.0, "rate");
        let Decl::Let {
            body: LetBody::Arms(arms),
            ..
        } = &prog.decls[1].0
        else {
            panic!("not a let with arms")
        };
        assert_eq!(arms.len(), 3);
        assert!(matches!(
            arms[0].when.as_ref().map(|w| w.0.as_ref()),
            Some(Expr::Num(_))
        ));
        assert!(matches!(
            arms[1].when.as_ref().map(|w| w.0.as_ref()),
            Some(Expr::Range { to: None, .. })
        ));
        assert!(matches!(&arms[1].growth, Some(g) if g.kind == GrowthKind::Compounding));
        assert!(arms[2].when.is_none());
    }

    #[test]
    fn parses_entries() {
        let prog = parse_prog(
            "entry paycheck \"Paycheck\" @ 15th | last {
               let gross = salary
               income.salary -> assets.cash : gross
               assets.cash   -> assets.retirement : min(gross * 16%, 1_000)  #contribution
             }
             entry interest after paycheck, loans.repay {
               liabilities.accrued -> expenses.interest : liabilities.loan * 5%/year
             }",
        );
        let Decl::Entry {
            name,
            label,
            when,
            body,
            ..
        } = &prog.decls[0].0
        else {
            panic!("not an entry")
        };
        assert_eq!(name.0, "paycheck");
        assert_eq!(label.as_deref(), Some("Paycheck"));
        assert!(matches!(
            when.as_ref().map(|w| w.0.as_ref()),
            Some(Expr::Bin(_, BinOp::Union, _))
        ));
        assert_eq!(body.len(), 3);
        let EntryStmt::Move(m) = &body[2] else {
            panic!("not a move")
        };
        assert_eq!(m.from.0.join(), "assets.cash");
        assert_eq!(m.to.0.join(), "assets.retirement");
        assert!(
            matches!(m.amount.0.as_ref(), Expr::Call(callee, args) if callee.join() == "min" && args.len() == 2)
        );
        assert_eq!(m.tag.as_ref().map(|t| t.0.as_str()), Some("contribution"));
        let Decl::Entry {
            label, when, after, ..
        } = &prog.decls[1].0
        else {
            panic!("not an entry")
        };
        assert!(label.is_none() && when.is_none());
        let after: Vec<String> = after.iter().map(|(p, _)| p.join()).collect();
        assert_eq!(after, ["paycheck", "loans.repay"]);
    }

    #[test]
    fn parses_checks_balances_and_modules() {
        let prog = parse_prog(
            "check assets.cash >= 0
             check ytd >= 1 @ 2026-12-31
             balances @ 2026-01-01 {
               assets.cash = 5_000
               liabilities.loan = 30_000
             }
             mod loans
             mod home { let rent = 3_000/month }",
        );
        assert!(matches!(&prog.decls[0].0, Decl::Check { when: None, .. }));
        assert!(matches!(
            &prog.decls[1].0,
            Decl::Check { when: Some(_), .. }
        ));
        let Decl::Balances { balances, .. } = &prog.decls[2].0 else {
            panic!("not balances")
        };
        assert_eq!(balances.len(), 2);
        assert!(matches!(
            &prog.decls[3].0,
            Decl::Mod {
                body: ModBody::File,
                ..
            }
        ));
        assert!(
            matches!(&prog.decls[4].0, Decl::Mod { body: ModBody::Inline(decls), .. } if decls.len() == 1)
        );
    }

    #[test]
    fn parses_functions_that_make_modules() {
        let prog = parse_prog(
            "fn loan(debt, rate) {
               let monthly = rate / 12
               mod { entry repay @ last { assets.cash -> debt : 1 } }
             }
             mod car = loan(debt: liabilities.car, rate: 5%)",
        );
        let Decl::Fn { body, .. } = &prog.decls[0].0 else {
            panic!("not a function")
        };
        assert!(matches!(&body[..], [Stmt::Let { .. }, Stmt::Mod(decls)] if decls.len() == 1));
        let Decl::Mod {
            body: ModBody::Call(call),
            ..
        } = &prog.decls[1].0
        else {
            panic!("not a module made by a call")
        };
        let Expr::Call(_, args) = call.0.as_ref() else {
            panic!("not a call")
        };
        assert!(matches!(args[0].0.as_ref(), Expr::Named((name, _), _) if name == "debt"));
        let errs = parse_errs("mod car = 5");
        assert!(errs[0].message.contains("expected a call"), "{errs:?}");
    }

    #[test]
    fn parses_functions() {
        let prog = parse_prog(
            "fn net(gross, rate) {
               let tax = gross * rate
               gross - tax
             }",
        );
        let Decl::Fn { params, body, .. } = &prog.decls[0].0 else {
            panic!("not a function")
        };
        assert_eq!(params.len(), 2);
        assert!(matches!(&body[..], [Stmt::Let { .. }, Stmt::Return(_)]));
    }

    #[test]
    fn calls_need_their_paren_right_after_the_name() {
        assert!(matches!(expr("min(a, b)"), Expr::Call(..)));
        // The value of the first arm doesn't call the second's days.
        let prog = parse_prog("let x { 2026 => prices\n (jan | feb) => 0 }");
        let Decl::Let {
            body: LetBody::Arms(arms),
            ..
        } = &prog.decls[0].0
        else {
            panic!()
        };
        assert_eq!(arms.len(), 2);
        let errs = parse_errs("fn f(x) { min (x, 1) }");
        assert!(
            errs[0]
                .extra
                .iter()
                .any(|(_, note)| note.contains("right after its name")),
            "{errs:?}"
        );
    }

    #[test]
    fn percent_divides_by_100() {
        assert!(matches!(expr("6.2%"), Expr::Num(n) if n == Decimal::new(62, 3)));
        assert!(matches!(expr("-5%"), Expr::Neg(_)));
        let Expr::Bin(x, BinOp::Div, hundred) = expr("rate%") else {
            panic!()
        };
        assert!(matches!(*x.0, Expr::Ref(_)));
        assert!(matches!(*hundred.0, Expr::Num(n) if n == Decimal::ONE_HUNDRED));
    }

    #[test]
    fn units_bind_like_division() {
        let Expr::Bin(_, BinOp::Sub, rhs) = expr("salary - 500/month") else {
            panic!()
        };
        assert!(matches!(*rhs.0, Expr::Per(_, TimeUnit::Month)));
        let Expr::Per(inner, TimeUnit::Year) = expr("(a + b)/year") else {
            panic!()
        };
        assert!(matches!(*inner.0, Expr::Bin(_, BinOp::Add, _)));
        let Expr::Bin(rate, BinOp::Mul, _) = expr("2_000/month * prices") else {
            panic!()
        };
        assert!(matches!(*rate.0, Expr::Per(_, TimeUnit::Month)));
        assert!(matches!(expr("x / months"), Expr::Bin(_, BinOp::Div, _)));
    }

    #[test]
    fn totals_take_a_unit_in_brackets() {
        let Expr::Period(inner, TimeUnit::Year) = expr("paycheck.contribution[year]") else {
            panic!()
        };
        assert!(matches!(&*inner.0, Expr::Ref(path) if path.join() == "paycheck.contribution"));
        let errs = parse_errs("check x[fortnight] > 0");
        assert!(errs[0].message.contains("after `[`"), "{errs:?}");
    }

    #[test]
    fn parses_day_literals() {
        let cases = [
            ("*", All),
            ("fri", Weekday(Dow::Friday)),
            ("weekday", Weekday(Dow::Weekday)),
            ("15th", MonthDay(Ordinal::Nth(15))),
            ("last", MonthDay(Ordinal::Last)),
            ("2nd mon", NthWeekday(Ordinal::Nth(2), Dow::Monday)),
            ("last fri", NthWeekday(Ordinal::Last, Dow::Friday)),
            ("jan", Month(Month::January)),
            ("jan 1", YearDay(Month::January, Ordinal::Nth(1))),
            ("jul 4th", YearDay(Month::July, Ordinal::Nth(4))),
            ("feb last", YearDay(Month::February, Ordinal::Last)),
            ("jul 2026", MonthYear(Month::July, 2026)),
            ("q2", Quarter(2)),
            ("q3 2027", QuarterYear(3, 2027)),
        ];
        for (src, lit) in cases {
            assert!(
                matches!(expr(src), Expr::Days(found) if found == lit),
                "{src}"
            );
        }
    }

    #[test]
    fn day_set_operators() {
        // `/` binds tighter than `&`, which binds tighter than `|`.
        let Expr::Bin(range, BinOp::Inter, thin) = expr("2026-01-02.. & fri / 2") else {
            panic!()
        };
        assert!(matches!(
            *range.0,
            Expr::Range {
                to: None,
                inclusive: false,
                ..
            }
        ));
        assert!(matches!(*thin.0, Expr::Bin(_, BinOp::Div, _)));
        let Expr::Bin(_, BinOp::Union, inter) = expr("15th | last & q2") else {
            panic!()
        };
        assert!(matches!(*inter.0, Expr::Bin(_, BinOp::Inter, _)));
        assert!(matches!(
            expr("sep..=jun"),
            Expr::Range {
                inclusive: true,
                to: Some(_),
                ..
            }
        ));
        assert!(matches!(expr("* / 3"), Expr::Bin(_, BinOp::Div, _)));
    }

    #[test]
    fn impossible_days_are_errors() {
        let error = |src: &str| parse_expression(src).unwrap_err()[0].message.clone();
        assert_eq!(error("32nd"), "no month has a 32nd day");
        assert_eq!(error("feb 30"), "February has at most 29 days");
        assert_eq!(error("6th mon"), "no month has a 6th Monday");
    }

    #[test]
    fn if_requires_then_and_else() {
        let errs = parse_errs("check if assets.cash > 0 1 else 0");
        assert!(errs.iter().any(|d| d.message.contains("then")));
        let errs = parse_errs("check if assets.cash > 0 then 1 0");
        assert!(errs.iter().any(|d| d.message.contains("else")));
    }

    #[test]
    fn malformed_declarations_are_never_dropped_silently() {
        let cases = [
            ("let 5 = 3", "expected a name after `let`"),
            ("fn 5(x) { x }", "expected a function name"),
            ("fn f(1) { 2 }", "expected a parameter name"),
            ("fn f(x) { let 5 = x\n x }", "expected a name after `let`"),
            ("fn f(x) { }", "needs a value to return"),
            ("entry e { assets.cash expenses.rent : 1 }", "expected `->`"),
            ("entry e { assets.cash -> expenses.rent 1 }", "expected `:`"),
            ("entry e { a -> b : 1 # }", "expected a tag"),
            ("entry e fri { }", "expected `{` to start the entry"),
            ("balances 2026-01-01 { }", "expected `@`"),
            ("let x { 2026 = 1 }", "expected `=>`"),
            ("check x\nlet", "expected a name after `let`"),
            ("assets.cash", "expected a declaration"),
        ];
        for (src, expected) in cases {
            let tokens = crate::lexer::lex(src, 0).expect("lexer errored");
            let Err(errs) = parse(tokens) else {
                panic!("`{src}` parsed without errors");
            };
            assert!(
                errs.iter().any(|d| d.message.contains(expected)),
                "`{src}`: expected an error containing {expected:?}, got {errs:?}"
            );
        }
    }
}
