use crate::Span;
use chrono::NaiveDate;
use chumsky::prelude::*;
use rust_decimal::Decimal;
use std::fmt;

#[derive(Clone, Debug, PartialEq)]
pub enum Token<'src> {
    Integer(i64),
    Float(Decimal),
    Date(NaiveDate),
    Ident(&'src str),
    Str(String),
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
}

impl<'src> fmt::Display for Token<'src> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Token::Integer(n) => write!(f, "{n}"),
            Token::Float(n) => write!(f, "{n}"),
            Token::Date(d) => write!(f, "{}", d.format("%Y-%m-%d")),
            Token::Ident(s) => write!(f, "{s}"),
            Token::Str(s) => write!(f, "\"{s}\""),
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

    let string = just('"')
        .ignore_then(
            choice((
                just('\\').ignore_then(choice((
                    just('"').to('"'),
                    just('\\').to('\\'),
                    just('n').to('\n'),
                    just('t').to('\t'),
                ))),
                any().filter(|c: &char| *c != '"' && *c != '\\'),
            ))
            .repeated()
            .collect::<String>(),
        )
        .then_ignore(just('"'))
        .map(Token::Str);

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

    // Line comment → silently dropped (newline consumed below with other whitespace).
    let comment = just("//")
        .then(any().and_is(text::newline().not()).repeated())
        .to(None::<Token<'src>>);

    // All whitespace (spaces, tabs, newlines) → silently dropped.
    let skip_ws = any()
        .filter(|c: &char| c.is_ascii_whitespace())
        .to(None::<Token<'src>>);

    choice((
        comment,
        date.map(Some),
        boolean.map(Some),
        num.map(Some),
        string.map(Some),
        ident.map(Some),
        punct.map(Some),
        skip_ws,
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
    fn whitespace_dropped() {
        // All whitespace — single spaces, multiple spaces, tabs, newlines — is ignored.
        let toks = lex("a  b\n\tc");
        assert_eq!(
            toks,
            vec![Token::Ident("a"), Token::Ident("b"), Token::Ident("c")]
        );
    }

    #[test]
    fn line_comment_drops_to_newline() {
        let toks = lex("// a comment\n42");
        assert_eq!(toks, vec![Token::Integer(42)]);
    }
}
