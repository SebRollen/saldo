use chrono::NaiveDate;
use chumsky::prelude::*;
use chumsky::span::SimpleSpan;
use rust_decimal::Decimal;
use std::fmt;

pub type Span = SimpleSpan<usize>;

#[derive(Clone, Debug, PartialEq)]
pub enum Token<'src> {
    Integer(i64),
    Float(Decimal),
    Date(NaiveDate),
    Ident(&'src str),
    True,
    False,
    Eq,
    EqEq,
    LtEq,
    GtEq,
    Plus,
    Minus,
    Star,
    Slash,
    Colon,
    Period,
    Comma,
    Percent,
    LessThan,
    MoreThan,
    LParen,
    RParen,
    LBrace,
    RBrace,
    Indent(usize),
    HardSpace,
}

impl<'src> fmt::Display for Token<'src> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Token::Integer(n) => write!(f, "{n}"),
            Token::Float(n) => write!(f, "{n}"),
            Token::Date(d) => write!(f, "{}", d.format("%Y-%m-%d")),
            Token::Ident(s) => write!(f, "{s}"),
            Token::True => write!(f, "true"),
            Token::False => write!(f, "false"),
            Token::Eq => write!(f, "="),
            Token::EqEq => write!(f, "=="),
            Token::LtEq => write!(f, "<="),
            Token::GtEq => write!(f, ">="),
            Token::Plus => write!(f, "+"),
            Token::Minus => write!(f, "-"),
            Token::Star => write!(f, "*"),
            Token::Slash => write!(f, "/"),
            Token::Colon => write!(f, ":"),
            Token::Period => write!(f, "."),
            Token::Comma => write!(f, ","),
            Token::Percent => write!(f, "%"),
            Token::LessThan => write!(f, "<"),
            Token::MoreThan => write!(f, ">"),
            Token::LParen => write!(f, "("),
            Token::RParen => write!(f, ")"),
            Token::LBrace => write!(f, "{{"),
            Token::RBrace => write!(f, "}}"),
            Token::Indent(n) => write!(f, "<indent:{n}>"),
            Token::HardSpace => write!(f, "<hardspace>"),
        }
    }
}

pub fn lexer<'src>(
) -> impl Parser<'src, &'src str, Vec<(Token<'src>, Span)>, extra::Err<Rich<'src, char, Span>>> {
    let digit = any().filter(|c: &char| c.is_ascii_digit());

    // @YYYY-MM-DD
    let at_date = just('@')
        .ignore_then(digit.repeated().exactly(4).collect::<String>())
        .then_ignore(just('-'))
        .then(digit.repeated().exactly(2).collect::<String>())
        .then_ignore(just('-'))
        .then(digit.repeated().exactly(2).collect::<String>())
        .try_map(|((y, m), d), span| {
            let y: i32 = y.parse().unwrap();
            let m: u32 = m.parse().unwrap();
            let d: u32 = d.parse().unwrap();
            NaiveDate::from_ymd_opt(y, m, d)
                .map(Token::Date)
                .ok_or_else(|| Rich::custom(span, format!("invalid date: {y:04}-{m:02}-{d:02}")))
        });

    // YYYY-MM-DD (bare, no @ prefix)
    let bare_date = digit
        .repeated()
        .exactly(4)
        .collect::<String>()
        .then_ignore(just('-'))
        .then(digit.repeated().exactly(2).collect::<String>())
        .then_ignore(just('-'))
        .then(digit.repeated().exactly(2).collect::<String>())
        .try_map(|((y, m), d), span| {
            let y: i32 = y.parse().unwrap();
            let m: u32 = m.parse().unwrap();
            let d: u32 = d.parse().unwrap();
            NaiveDate::from_ymd_opt(y, m, d)
                .map(Token::Date)
                .ok_or_else(|| Rich::custom(span, format!("invalid date: {y:04}-{m:02}-{d:02}")))
        });

    let date = choice((at_date, bare_date));

    let digits_with_underscores = any()
        .filter(|c: &char| c.is_ascii_digit() || *c == '_')
        .repeated()
        .at_least(1)
        .collect::<String>();

    let num = digits_with_underscores
        .then(just('.').ignore_then(digits_with_underscores).or_not())
        .map(|(int_part, frac_part)| {
            let clean_int: String = int_part.chars().filter(|c| *c != '_').collect();
            let clean_frac: Option<String> =
                frac_part.map(|f: String| f.chars().filter(|c| *c != '_').collect());
            match clean_frac {
                Some(frac) => Token::Float(
                    format!("{clean_int}.{frac}")
                        .parse()
                        .unwrap_or(Decimal::ZERO),
                ),
                None => Token::Integer(clean_int.parse().unwrap_or(0)),
            }
        });

    let ident = text::ascii::ident().map(Token::Ident);

    let punct = choice((
        just("==").to(Token::EqEq),
        just("<=").to(Token::LtEq),
        just(">=").to(Token::GtEq),
        just('=').to(Token::Eq),
        just('+').to(Token::Plus),
        just('-').to(Token::Minus),
        just('*').to(Token::Star),
        just('/').to(Token::Slash),
        just(':').to(Token::Colon),
        just('.').to(Token::Period),
        just(',').to(Token::Comma),
        just('%').to(Token::Percent),
        just('<').to(Token::LessThan),
        just('>').to(Token::MoreThan),
        just('(').to(Token::LParen),
        just(')').to(Token::RParen),
        just('{').to(Token::LBrace),
        just('}').to(Token::RBrace),
    ));

    let boolean = choice((just("false").to(Token::False), just("true").to(Token::True)));

    // Newline + leading whitespace → Indent(n). Each space or tab counts as 1.
    let indent = text::newline()
        .ignore_then(
            choice((just(' ').to(1usize), just('\t').to(1usize)))
                .repeated()
                .collect::<Vec<usize>>(),
        )
        .map(|counts| Token::Indent(counts.len()));

    // Tab or 2+ consecutive spaces within a line → HardSpace.
    let hard_space = choice((
        just('\t').to(()),
        just(' ')
            .repeated()
            .at_least(2)
            .collect::<Vec<char>>()
            .to(()),
    ))
    .to(Token::HardSpace);

    // Single space → silently dropped.
    let skip_space = just(' ').to(None::<Token<'src>>);

    // Line comment → silently dropped (newline is NOT consumed).
    let comment = just("//")
        .then(any().and_is(text::newline().not()).repeated())
        .to(None::<Token<'src>>);

    choice((
        comment,
        date.map(Some),
        boolean.map(Some),
        num.map(Some),
        ident.map(Some),
        punct.map(Some),
        indent.map(Some),
        hard_space.map(Some),
        skip_space,
    ))
    .map_with(|opt, e| opt.map(|t| (t, e.span())))
    .repeated()
    .collect::<Vec<_>>()
    .map(|v| v.into_iter().flatten().collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lex(src: &str) -> Vec<Token<'_>> {
        let (toks, errs) = lexer().parse(src).into_output_errors();
        assert!(errs.is_empty(), "lex errors: {errs:?}");
        toks.unwrap().into_iter().map(|(t, _)| t).collect()
    }

    #[test]
    fn lexes_at_date() {
        let toks = lex("@2026-01-01");
        assert_eq!(
            toks,
            vec![Token::Date(NaiveDate::from_ymd_opt(2026, 1, 1).unwrap())]
        );
    }

    #[test]
    fn lexes_bare_date() {
        let toks = lex("2026-01-01");
        assert_eq!(
            toks,
            vec![Token::Date(NaiveDate::from_ymd_opt(2026, 1, 1).unwrap())]
        );
    }

    #[test]
    fn lexes_multi_char_ops() {
        let toks = lex(">= <= ==");
        assert_eq!(toks, vec![Token::GtEq, Token::LtEq, Token::EqEq]);
    }

    #[test]
    fn lexes_numbers_with_underscores() {
        // Single spaces between tokens are dropped.
        let toks = lex("300_000 0.05 120_000.50");
        assert_eq!(
            toks,
            vec![
                Token::Integer(300_000),
                Token::Float(Decimal::new(5, 2)),
                Token::Float(Decimal::new(120_00050, 2)),
            ]
        );
    }

    #[test]
    fn lexes_numbers_with_hard_space() {
        // Double spaces produce a HardSpace token.
        let toks = lex("300_000  0.05");
        assert_eq!(
            toks,
            vec![
                Token::Integer(300_000),
                Token::HardSpace,
                Token::Float(Decimal::new(5, 2)),
            ]
        );
    }

    #[test]
    fn lexes_indent() {
        let toks = lex("foo\n  bar");
        assert_eq!(
            toks,
            vec![Token::Ident("foo"), Token::Indent(2), Token::Ident("bar"),]
        );
    }

    #[test]
    fn lexes_hard_space() {
        let toks = lex("a  b");
        assert_eq!(
            toks,
            vec![Token::Ident("a"), Token::HardSpace, Token::Ident("b"),]
        );
    }

    #[test]
    fn single_space_dropped() {
        let toks = lex("a b");
        assert_eq!(toks, vec![Token::Ident("a"), Token::Ident("b"),]);
    }

    #[test]
    fn line_comment_drops_to_newline() {
        let toks = lex("// a comment\n42");
        assert_eq!(toks, vec![Token::Indent(0), Token::Integer(42)]);
    }
}
