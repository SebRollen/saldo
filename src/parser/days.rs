//! Sets of days written as literals: `fri`, `15th`, `2nd mon`, `jan 1`,
//! `jul 2026`, `q2`, `*` and dates.

use super::Parser;
use crate::ast::schedule::quarter_from_name;
use crate::ast::{DayLit, Dow, Month, Ordinal};
use crate::lexer::Token;
use rust_decimal::Decimal;

impl Parser<'_> {
    /// A day literal, if the next token starts one: `None` if it doesn't, and
    /// `Some(None)` if it has an error.
    pub(super) fn try_parse_days(&mut self) -> Option<Option<DayLit>> {
        let start = self.peek_span();
        match self.peek() {
            Token::Date(d) => {
                let d = *d;
                self.advance();
                Some(Some(DayLit::Date(d)))
            }
            Token::Star => {
                self.advance();
                Some(Some(DayLit::All))
            }
            Token::Ordinal(n) => {
                let n = *n;
                self.advance();
                Some(self.month_occurrence(Ordinal::Nth(n), start))
            }
            Token::Ident(word) => {
                let word = *word;
                if word == "last" {
                    self.advance();
                    return Some(self.month_occurrence(Ordinal::Last, start));
                }
                if let Some(dow) = Dow::from_name(word) {
                    self.advance();
                    return Some(Some(DayLit::Weekday(dow)));
                }
                if let Some(month) = Month::from_name(word) {
                    self.advance();
                    return Some(self.after_month(month, start));
                }
                if let Some(quarter) = quarter_from_name(word) {
                    self.advance();
                    return Some(Some(match self.try_year() {
                        Some(year) => DayLit::QuarterYear(quarter, year),
                        None => DayLit::Quarter(quarter),
                    }));
                }
                None
            }
            _ => None,
        }
    }

    /// `15th` or `last`, and a weekday if one follows: that day or weekday of
    /// every month.
    fn month_occurrence(&mut self, ordinal: Ordinal, start: crate::Span) -> Option<DayLit> {
        if let Token::Ident(word) = self.peek()
            && let Some(dow) = Dow::from_name(word)
        {
            self.advance();
            if let Ordinal::Nth(n) = ordinal
                && n > dow.max_per_month()
            {
                self.error_at(start, format!("no month has a {ordinal} {}", dow.name()));
                return None;
            }
            return Some(DayLit::NthWeekday(ordinal, dow));
        }
        if let Ordinal::Nth(n) = ordinal
            && !(1..=31).contains(&n)
        {
            self.error_at(start, format!("no month has a {ordinal} day"));
            return None;
        }
        Some(DayLit::MonthDay(ordinal))
    }

    /// What follows a month's name: a day (`jan 1`, `jan 1st`, `feb last`), a
    /// year (`jul 2026`), or nothing (`jan`).
    fn after_month(&mut self, month: Month, start: crate::Span) -> Option<DayLit> {
        let day = match self.peek() {
            Token::Ordinal(n) => Some(Ordinal::Nth(*n)),
            Token::Number(n) if n.scale() == 0 && *n >= Decimal::ONE && *n <= Decimal::from(31) => {
                Some(Ordinal::Nth(
                    u8::try_from(n.mantissa()).expect("at most 31"),
                ))
            }
            Token::Ident("last") => Some(Ordinal::Last),
            _ => None,
        };
        if let Some(day) = day {
            self.advance();
            if let Ordinal::Nth(n) = day
                && n > month.max_days()
            {
                self.error_at(
                    start,
                    format!("{month:?} has at most {} days", month.max_days()),
                );
                return None;
            }
            return Some(DayLit::YearDay(month, day));
        }
        Some(match self.try_year() {
            Some(year) => DayLit::MonthYear(month, year),
            None => DayLit::Month(month),
        })
    }

    /// A year written in full, like `2026`, if one comes next.
    fn try_year(&mut self) -> Option<i32> {
        let Token::Number(n) = self.peek() else {
            return None;
        };
        let year = (n.scale() == 0)
            .then(|| i32::try_from(n.mantissa()).ok())
            .flatten()
            .filter(|y| (1000..=9999).contains(y))?;
        self.advance();
        Some(year)
    }

    fn error_at(&mut self, start: crate::Span, message: String) {
        let span = crate::Span::new(start.start, self.last_span.end);
        self.errors
            .push(crate::errors::Diagnostic::new(span, message));
    }
}
