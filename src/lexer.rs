use crate::ast::{Spanned, Span};
use crate::errors::Diagnostic;
use chrono::NaiveDate;
use rust_decimal::Decimal;
use std::fmt;

#[derive(Clone, Debug, PartialEq)]
pub enum Token<'src> {
    Float(Decimal),
    Date(NaiveDate),
    Ident(&'src str),
    Str(&'src str),
    Ordinal(u8),
    
    // keywords
    True,
    False,
    Assert,
    Entry,
    Param,
    Schedule,

    // punctuation
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
    Lt,
    Gt,
    LParen,
    RParen,
    LBrace,
    RBrace,
    EOF,
}

impl<'src> fmt::Display for Token<'src> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Token::Float(n) => write!(f, "{n}"),
            Token::Date(d) => write!(f, "{}", d.format("%Y-%m-%d")),
            Token::Ident(s) => write!(f, "{s}"),
            Token::Str(s) => write!(f, "\"{s}\""),
            Token::Ordinal(i) => {
                let suffix = if *i == 11 || *i == 12 || *i == 13 {
                    "th"
                } else {
                    match i % 10 {
                        1 => "st",
                        2 => "nd",
                        3 => "rd",
                        _ => "th",
                    }
                };
                write!(f, "{i}{suffix}")
            }
            Token::True => write!(f, "true"),
            Token::False => write!(f, "false"),
            Token::Assert => write!(f, "assert"),
            Token::Entry => write!(f, "entry"),
            Token::Param => write!(f, "param"),
            Token::Schedule => write!(f, "schedule"),
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
            Token::Lt => write!(f, "<"),
            Token::Gt => write!(f, ">"),
            Token::LParen => write!(f, "("),
            Token::RParen => write!(f, ")"),
            Token::LBrace => write!(f, "{{"),
            Token::RBrace => write!(f, "}}"),
            Token::EOF => write!(f, "EOF"),
        }
    }
}

pub struct Lexer<'src> {
    src: &'src str,
    bytes: &'src [u8],
    start: usize,
    current: usize,
}

impl<'src> Lexer<'src> {
    pub fn new(src: &'src str) -> Self {
        Self {
            src,
            bytes: src.as_bytes(),
            start: 0,
            current: 0,
        }
    }

    pub fn lex(&mut self) -> Result<Vec<Spanned<Token<'src>>>, Vec<Diagnostic>> {
        let mut tokens = Vec::new();
        let mut errors = Vec::new();
        loop {
            match self.lex_token() {
                Ok((Token::EOF, _)) => {
                    if errors.is_empty() {
                        return Ok(tokens)
                    } else {
                        return Err(errors)
                    }
                },
                Ok(token) => tokens.push(token),
                Err(diagnostic) => errors.push(diagnostic),
            }
        }
    }

    fn current_span(&self) -> Span {
        Span::new(self.start, self.current)
    }

    fn peek(&self) -> Option<&u8> {
        self.bytes.get(self.current)
    }

    fn peek_next(&self) -> Option<u8> {
        if self.current + 1 >= self.src.len() {
            return None;
        }
        Some(self.bytes[self.current + 1])
    }

    fn advance(&mut self) -> u8 {
        self.current += 1;
        self.bytes[self.current - 1]
    }

    fn matches(&mut self, expected: u8) -> bool {
        if self.at_end() {
            return false;
        }

        if *self.peek().expect("already checked at_end") != expected {
            return false;
        }
        self.current += 1;
        true
    }

    fn skip_whitespace(&mut self) {
        loop {
            match self.peek() {
                Some(b' ') | Some(b'\r') | Some(b'\t') | Some(b'\n') => {
                    self.advance();
                }
                Some(b'/') => {
                    if self.peek_next() == Some(b'/') {
                        // entering a comment, skip to end of line
                        while let Some(s) = self.peek() && *s != b'\n' && !self.at_end() {
                            self.advance();
                        }
                    } else {
                        return;
                    }
                }
                _ => return,
            }
        }
    }

    fn at_end(&self) -> bool {
        self.current >= self.src.len()
    }

    fn emit_token(&self, t: Token<'src>) -> Result<Spanned<Token<'src>>, Diagnostic> {
        Ok((t, self.current_span()))
    }

    fn lex_string(&mut self) -> Result<Spanned<Token<'src>>, Diagnostic> {
        loop {
            if self.at_end() {
                return Err(Diagnostic::new(self.current_span(), "Unterminated string."));
            }

            if *self.peek().expect("already checked at_end") == b'"' {
                self.advance();
                break;
            }
            self.advance();
        }

        // guaranteed to be a char index boundary since we just progressed past "
        let content = &self.src[self.start+1..self.current-1];
        self.emit_token(Token::Str(content))
    }

    fn lex_digitlike(&mut self) -> Result<Spanned<Token<'src>>, Diagnostic> {
        // The first digit was already consumed; self.start is its position.

        // Try bare date: YYYY-MM-DD (checks from self.start without moving current)
        if let Some(date) = self.try_lex_bare_date() {
            self.current = self.start + 10;
            return self.emit_token(Token::Date(date));
        }

        // Consume remaining plain digits (no underscores) for ordinal probe
        while matches!(self.peek(), Some(c) if c.is_ascii_digit()) {
            self.advance();
        }
        let digit_str = &self.src[self.start..self.current];

        // Check for ordinal suffix (st/nd/rd/th) not immediately followed by a word char
        if let Some(suffix_len) = self.ordinal_suffix_len() {
            let after = self.current + suffix_len;
            let next_is_word = self.bytes
                .get(after)
                .is_some_and(|b| b.is_ascii_alphanumeric() || *b == b'_');
            if !next_is_word {
                self.current = after;
                return match digit_str.parse::<u8>() {
                    Ok(n) => self.emit_token(Token::Ordinal(n)),
                    Err(_) => Err(Diagnostic::new(
                        self.current_span(),
                        format!("ordinal out of range: {digit_str}"),
                    )),
                };
            }
        }

        // Number: reset and re-consume digits+underscores for the integer part
        self.current = self.start;
        while matches!(self.peek(), Some(c) if c.is_ascii_digit() || *c == b'_') {
            self.advance();
        }
        let int_str: String = self.src[self.start..self.current]
            .bytes()
            .filter(|b| *b != b'_')
            .map(|b| b as char)
            .collect();

        // Optional fractional part: only consume '.' if a digit/underscore follows
        let num_str = if matches!(self.peek(), Some(b'.'))
            && matches!(self.peek_next(), Some(c) if c.is_ascii_digit() || c == b'_')
        {
            self.advance(); // consume '.'
            let frac_start = self.current;
            while matches!(self.peek(), Some(c) if c.is_ascii_digit() || *c == b'_') {
                self.advance();
            }
            let frac_str: String = self.src[frac_start..self.current]
                .bytes()
                .filter(|b| *b != b'_')
                .map(|b| b as char)
                .collect();
            format!("{int_str}.{frac_str}")
        } else {
            int_str
        };

        let val = num_str.parse::<Decimal>().unwrap_or(Decimal::ZERO);
        self.emit_token(Token::Float(val))
    }

    fn lex_identifier(&mut self) -> Result<Spanned<Token<'src>>, Diagnostic> {
        loop {
            match self.peek() {
                Some(c) if c.is_ascii_alphanumeric() || *c == b'_' => {
                    self.advance();
                },
                _ => break
            }

        }

        let word = &self.src[self.start..self.src.ceil_char_boundary(self.current)];
        let token = match word {
            "true" => Token::True,
            "false" => Token::False,
            "assert" => Token::Assert,
            "entry" => Token::Entry,
            "param" => Token::Param,
            "schedule" => Token::Schedule,
            _ => Token::Ident(word),
        };

        self.emit_token(token)
    }

    // Returns the length (always 2) of an ordinal suffix at self.current, or None.
    // Callers are responsible for checking that the following char is not a word char.
    fn ordinal_suffix_len(&self) -> Option<usize> {
        let i = self.current;
        if i + 2 > self.src.len() {
            return None;
        }
        match &self.src[i..i + 2] {
            "st" | "nd" | "rd" | "th" => Some(2),
            _ => None,
        }
    }

    // Checks whether a YYYY-MM-DD date starts at self.start, and if so parses and
    // returns it. Does not advance self.current.
    fn try_lex_bare_date(&self) -> Option<NaiveDate> {
        let i = self.start;
        let b = self.bytes;
        if i + 10 > self.src.len() {
            return None;
        }
        if !b[i..i + 4].iter().all(|c| c.is_ascii_digit()) {
            return None;
        }
        if b[i + 4] != b'-' {
            return None;
        }
        if !b[i + 5..i + 7].iter().all(|c| c.is_ascii_digit()) {
            return None;
        }
        if b[i + 7] != b'-' {
            return None;
        }
        if !b[i + 8..i + 10].iter().all(|c| c.is_ascii_digit()) {
            return None;
        }
        // Reject if immediately followed by another digit (part of a longer number)
        if b.get(i + 10).is_some_and(|c| c.is_ascii_digit()) {
            return None;
        }
        let y: i32 = self.src[i..i + 4].parse().ok()?;
        let m: u32 = self.src[i + 5..i + 7].parse().ok()?;
        let d: u32 = self.src[i + 8..i + 10].parse().ok()?;
        NaiveDate::from_ymd_opt(y, m, d)
    }

    fn lex_token(&mut self) -> Result<Spanned<Token<'src>>, Diagnostic> {
        self.skip_whitespace();
        self.start = self.current;

        if self.at_end() {
            return self.emit_token(Token::EOF);
        }

        let c = self.advance();

        if c.is_ascii_alphabetic() || c == b'_' {
            return self.lex_identifier();
        }

        if c.is_ascii_digit() {
            return self.lex_digitlike();
        }

        match c {
            b'+' => return self.emit_token(Token::Plus),
            b'-' => return self.emit_token(Token::Minus),
            b'*' => return self.emit_token(Token::Star),
            b'/' => return self.emit_token(Token::Slash),
            b':' => return self.emit_token(Token::Colon),
            b'.' => return self.emit_token(Token::Period),
            b',' => return self.emit_token(Token::Comma),
            b'%' => return self.emit_token(Token::Percent),
            b'(' => return self.emit_token(Token::LParen),
            b')' => return self.emit_token(Token::RParen),
            b'{' => return self.emit_token(Token::LBrace),
            b'}' => return self.emit_token(Token::RBrace),
            b'=' => {
                if self.matches(b'=') {
                    return self.emit_token(Token::EqEq);
                } else {
                    return self.emit_token(Token::Eq);
                }
            }
            b'<' => {
                if self.matches(b'=') {
                    return self.emit_token(Token::LtEq);
                } else {
                    return self.emit_token(Token::Lt);
                }
            }
            b'>' => {
                if self.matches(b'=') {
                    return self.emit_token(Token::GtEq);
                } else {
                    return self.emit_token(Token::Gt);
                }
            }
            b'"' => return self.lex_string(),
            _ => {}
        }

        Err(Diagnostic::new(
            Span::new(self.start, self.current),
            "Unexpected character",
        ))
    }
}


#[cfg(test)]
mod tests {
    use super::*;

    fn lex(src: &str) -> Vec<Token<'_>> {
        let Ok(toks) = Lexer::new(src).lex() else {
            panic!("lexer errored")
        };
        toks.into_iter().map(|(t, _)| t).collect()
    }

    #[test]
    fn lexes_date() {
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
        let toks = lex("300_000 0.05 120_000.50");
        assert_eq!(
            toks,
            vec![
                Token::Float(Decimal::new(300_000, 0)),
                Token::Float(Decimal::new(5, 2)),
                Token::Float(Decimal::new(12000050, 2)),
            ]
        );
    }

    #[test]
    fn whitespace_dropped() {
        let toks = lex("a  b\n\tc");
        assert_eq!(
            toks,
            vec![Token::Ident("a"), Token::Ident("b"), Token::Ident("c")]
        );
    }

    #[test]
    fn line_comment_drops_to_newline() {
        let toks = lex("// a comment\n42");
        assert_eq!(toks, vec![Token::Float(Decimal::new(42, 0))]);
    }
}
