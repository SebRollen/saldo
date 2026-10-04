use super::Parser;
use crate::Span;
use crate::ast::schedule::{
    Dow, Month, MonthOccurrence, Nth, Ordinal, Period, Periodic, Schedule, When, Window,
};
use crate::errors::Diagnostic;
use crate::lexer::Token;
use chrono::NaiveDate;
use rust_decimal::Decimal;

impl<'src> Parser<'src> {
    pub(super) fn parse_schedule_literal(&mut self) -> Option<Schedule> {
        match self.peek() {
            Token::Ident(s) => {
                let s = *s;
                if s == "every" {
                    self.parse_periodic()
                } else if matches!(
                    s,
                    "daily" | "weekly" | "monthly" | "quarterly" | "yearly" | "annually"
                ) {
                    self.parse_adverbial()
                } else {
                    None
                }
            }
            Token::Date(_) => self.parse_date_list().map(Schedule::Dates),
            _ => None,
        }
    }

    fn parse_periodic(&mut self) -> Option<Schedule> {
        self.eat_keyword("every")?;
        let nth = self.try_parse_nth();
        let period = self.parse_period()?;
        let within = self.parse_within()?;
        let start = if self.eat_keyword("from").is_some() {
            Some(self.parse_date()?)
        } else {
            None
        };
        Some(Schedule::Periodic(Periodic {
            nth,
            period,
            within,
            start,
        }))
    }

    fn parse_adverbial(&mut self) -> Option<Schedule> {
        let Token::Ident(s) = self.peek() else {
            return None;
        };
        let period = match *s {
            "daily" => {
                self.advance();
                Period::Day
            }
            "weekly" => {
                self.advance();
                let on = if self.eat_keyword("on").is_some() {
                    self.parse_dow_list()
                } else {
                    Vec::new()
                };
                Period::Week { on }
            }
            "monthly" => {
                self.advance();
                let on = if self.eat_keyword("on").is_some() {
                    self.eat_keyword("the");
                    self.parse_month_occurrence_list()
                } else {
                    Vec::new()
                };
                Period::Month { on }
            }
            "quarterly" => {
                self.advance();
                Period::Quarter
            }
            "yearly" | "annually" => {
                self.advance();
                let on = if self.eat_keyword("on").is_some() {
                    self.parse_year_occurrence_list()
                } else {
                    Vec::new()
                };
                Period::Year { on }
            }
            _ => return None,
        };
        let within = self.parse_within()?;
        Some(Schedule::Periodic(Periodic {
            nth: None,
            period,
            within,
            start: None,
        }))
    }

    fn parse_period(&mut self) -> Option<Period> {
        if let Some(p) = self.try_parse_day() {
            return Some(p);
        }
        if let Some(d) = self.try_parse_dow() {
            return Some(Period::Weekday(d));
        }
        if let Some(p) = self.try_parse_week() {
            return Some(p);
        }
        if let Some(p) = self.try_parse_month_period() {
            return Some(p);
        }
        if let Some(p) = self.try_parse_quarter() {
            return Some(p);
        }
        if let Some(p) = self.try_parse_year() {
            return Some(p);
        }
        let span = self.peek_span();
        self.errors.push(Diagnostic::new(
            span,
            "expected period (day, week, month, quarter, year, or day-of-week)",
        ));
        None
    }

    fn try_parse_day(&mut self) -> Option<Period> {
        if let Token::Ident(s) = self.peek()
            && (*s == "day" || *s == "days")
        {
            self.advance();
            return Some(Period::Day);
        }
        None
    }

    fn try_parse_week(&mut self) -> Option<Period> {
        if let Token::Ident(s) = self.peek()
            && (*s == "week" || *s == "weeks")
        {
            self.advance();
            let on = if self.eat_keyword("on").is_some() {
                self.parse_dow_list()
            } else {
                Vec::new()
            };
            return Some(Period::Week { on });
        }
        None
    }

    fn try_parse_month_period(&mut self) -> Option<Period> {
        if let Some(month) = self.try_parse_month_name() {
            let start = self.peek_span();
            let day = self.try_parse_ordinal();
            if let Some(day) = &day {
                self.check_day_ordinal(day, Some(month), start);
            }
            return Some(Period::NamedMonth { month, day });
        }
        if let Token::Ident(s) = self.peek()
            && (*s == "month" || *s == "months")
        {
            self.advance();
            let on = if self.eat_keyword("on").is_some() {
                self.eat_keyword("the");
                self.parse_month_occurrence_list()
            } else {
                Vec::new()
            };
            return Some(Period::Month { on });
        }
        None
    }

    fn try_parse_quarter(&mut self) -> Option<Period> {
        if let Token::Ident(s) = self.peek()
            && (*s == "quarter" || *s == "quarters")
        {
            self.advance();
            return Some(Period::Quarter);
        }
        None
    }

    fn try_parse_year(&mut self) -> Option<Period> {
        if let Token::Ident(s) = self.peek()
            && (*s == "year" || *s == "years")
        {
            self.advance();
            let on = if self.eat_keyword("on").is_some() {
                self.parse_year_occurrence_list()
            } else {
                Vec::new()
            };
            return Some(Period::Year { on });
        }
        None
    }

    fn try_parse_dow(&mut self) -> Option<Dow> {
        let Token::Ident(s) = self.peek() else {
            return None;
        };
        let dow = match *s {
            "monday" | "mondays" | "mon" => Dow::Monday,
            "tuesday" | "tuesdays" | "tue" => Dow::Tuesday,
            "wednesday" | "wednesdays" | "wed" => Dow::Wednesday,
            "thursday" | "thursdays" | "thu" => Dow::Thursday,
            "friday" | "fridays" | "fri" => Dow::Friday,
            "saturday" | "saturdays" | "sat" => Dow::Saturday,
            "sunday" | "sundays" | "sun" => Dow::Sunday,
            "weekday" | "weekdays" => Dow::Weekday,
            "weekend" => Dow::Weekend,
            _ => return None,
        };
        self.advance();
        Some(dow)
    }

    fn try_parse_month_name(&mut self) -> Option<Month> {
        let Token::Ident(s) = self.peek() else {
            return None;
        };
        let month = match *s {
            "january" | "jan" => Month::January,
            "february" | "feb" => Month::February,
            "march" | "mar" => Month::March,
            "april" | "apr" => Month::April,
            "may" => Month::May,
            "june" | "jun" => Month::June,
            "july" | "jul" => Month::July,
            "august" | "aug" => Month::August,
            "september" | "sep" => Month::September,
            "october" | "oct" => Month::October,
            "november" | "nov" => Month::November,
            "december" | "dec" => Month::December,
            _ => return None,
        };
        self.advance();
        Some(month)
    }

    fn try_parse_nth(&mut self) -> Option<Nth> {
        if let Token::Ordinal(n) = self.peek()
            && *n >= 2
        {
            let n = *n;
            self.advance();
            return Some(Nth::new(n));
        }
        if let Token::Number(n) = self.peek() {
            let n = *n;
            if n.fract() == Decimal::ZERO
                && n >= Decimal::from(2)
                && n <= Decimal::from(255)
                && let Ok(val) = n.to_string().parse::<u8>()
            {
                self.advance();
                return Some(Nth::new(val));
            }
        }
        if let Token::Ident(s) = self.peek() {
            let n: u8 = match *s {
                "second" => 2,
                "third" => 3,
                "fourth" => 4,
                "fifth" => 5,
                "sixth" => 6,
                "seventh" => 7,
                "eighth" => 8,
                "ninth" => 9,
                "tenth" => 10,
                _ => return None,
            };
            self.advance();
            return Some(Nth::new(n));
        }
        None
    }

    fn try_parse_ordinal(&mut self) -> Option<Ordinal> {
        if let Token::Ordinal(1) = self.peek() {
            self.advance();
            return Some(Ordinal::First);
        }
        // A bare 1 is a day, as in `jan 1`. `try_parse_nth` doesn't take it,
        // because as a count (`every 1 week`) it means nothing.
        if let Token::Number(n) = self.peek()
            && *n == Decimal::ONE
        {
            self.advance();
            return Some(Ordinal::First);
        }
        if let Token::Ident(s) = self.peek() {
            if *s == "first" {
                self.advance();
                return Some(Ordinal::First);
            }
            if *s == "last" {
                self.advance();
                return Some(Ordinal::Last);
            }
        }
        self.try_parse_nth().map(Ordinal::Nth)
    }

    fn require_ordinal(&mut self) -> Option<Ordinal> {
        if let Some(o) = self.try_parse_ordinal() {
            return Some(o);
        }
        let span = self.peek_span();
        self.errors.push(Diagnostic::new(span, "expected ordinal"));
        None
    }

    fn parse_dow_list(&mut self) -> Vec<Dow> {
        self.parse_comma_list("day of the week", |p| p.try_parse_dow())
    }

    fn parse_month_occurrence_list(&mut self) -> Vec<MonthOccurrence> {
        self.parse_comma_list("day of the month (like `15th` or `last friday`)", |p| {
            p.try_parse_month_occurrence()
        })
    }

    fn try_parse_month_occurrence(&mut self) -> Option<MonthOccurrence> {
        let start = self.peek_span();
        let ordinal = self.try_parse_ordinal()?;
        if let Some(dow) = self.try_parse_dow() {
            self.check_weekday_ordinal(&ordinal, &dow, start);
            return Some(MonthOccurrence::Weekday(ordinal, dow));
        }
        self.check_day_ordinal(&ordinal, None, start);
        let _ = self.eat_keyword("day").or_else(|| self.eat_keyword("days"));
        Some(MonthOccurrence::Day(ordinal))
    }

    /// Reports day ordinals that no month (or not `month`) has, such as "32nd".
    /// `start` is the span of the ordinal's first token.
    fn check_day_ordinal(&mut self, ordinal: &Ordinal, month: Option<Month>, start: Span) {
        let Ordinal::Nth(nth) = ordinal else { return };
        let max = month.map_or(31, Month::max_days);
        if nth.get() > max {
            let message = match month {
                Some(month) => format!("{month:?} has at most {max} days"),
                None => format!("no month has a {nth} day"),
            };
            let span = Span::new(start.start, self.last_span.end);
            self.errors.push(Diagnostic::new(span, message));
        }
    }

    /// Reports weekday ordinals that no month has, such as "6th monday".
    /// `start` is the span of the ordinal's first token.
    fn check_weekday_ordinal(&mut self, ordinal: &Ordinal, dow: &Dow, start: Span) {
        let Ordinal::Nth(nth) = ordinal else { return };
        if nth.get() > dow.max_per_month() {
            let span = Span::new(start.start, self.last_span.end);
            self.errors.push(Diagnostic::new(
                span,
                format!("no month has a {nth} {}", dow.name()),
            ));
        }
    }

    fn parse_year_occurrence_list(&mut self) -> Vec<(Month, Ordinal)> {
        self.parse_comma_list("month and day (like `jan 1st`)", |p| {
            p.try_parse_year_occurrence()
        })
    }

    fn try_parse_year_occurrence(&mut self) -> Option<(Month, Ordinal)> {
        let month = self.try_parse_month_name()?;
        let start = self.peek_span();
        let ordinal = self.require_ordinal()?;
        self.check_day_ordinal(&ordinal, Some(month), start);
        Some((month, ordinal))
    }

    fn parse_date_list(&mut self) -> Option<Vec<NaiveDate>> {
        if !matches!(self.peek(), Token::Date(_)) {
            return None;
        }
        Some(self.parse_comma_list("date", |p| {
            if let Token::Date(d) = p.peek() {
                let d = *d;
                p.advance();
                Some(d)
            } else {
                None
            }
        }))
    }

    pub(super) fn parse_date(&mut self) -> Option<NaiveDate> {
        if let Token::Date(d) = self.peek() {
            let d = *d;
            self.advance();
            Some(d)
        } else {
            let span = self.peek_span();
            self.errors.push(Diagnostic::new(span, "expected date"));
            None
        }
    }

    /// The days a param arm covers: `else`, `from <start> [to <date>]`, or
    /// `in` and a list of months, quarters and years.
    pub(super) fn parse_when(&mut self) -> Option<When> {
        if self.eat_keyword("else").is_some() {
            return Some(When::Always);
        }
        if self.eat_keyword("from").is_some() {
            let from = self.parse_start()?;
            let to = if self.eat_keyword("to").is_some() {
                Some(self.parse_date()?)
            } else {
                None
            };
            return Some(When::Windows(vec![Window::Dates(from, to)]));
        }
        if let Some(within) = self.try_parse_in() {
            return within;
        }
        let span = self.peek_span();
        self.errors
            .push(Diagnostic::new(span, "expected `from`, `in` or `else`"));
        None
    }

    /// `in` and a list of months, quarters and years, if the next word is
    /// `in`: `None` if it isn't, and `Some(None)` if the list has an error.
    fn try_parse_in(&mut self) -> Option<Option<When>> {
        self.eat_keyword("in")?;
        let listed = self.parse_comma_list(
            "a month, quarter or year (like `may`, `q2` or `2026`)",
            |p| p.try_parse_window(),
        );
        if listed.is_empty() {
            return Some(None);
        }
        // Every year's months go in one window, first.
        let months = listed.iter().fold(0, |mask, w| match w {
            Window::Months(m) => mask | m,
            Window::Dates(..) => mask,
        });
        let windows = (months != 0)
            .then_some(Window::Months(months))
            .into_iter()
            .chain(
                listed
                    .into_iter()
                    .filter(|w| matches!(w, Window::Dates(..))),
            )
            .collect();
        Some(Some(When::Windows(windows)))
    }

    /// The days a schedule keeps: an optional `in` list, or every day.
    fn parse_within(&mut self) -> Option<When> {
        self.try_parse_in().unwrap_or(Some(When::Always))
    }

    /// A `from`'s first day: a date, or the start of a year, or of a month or
    /// quarter in one.
    fn parse_start(&mut self) -> Option<NaiveDate> {
        if let Token::Date(_) = self.peek() {
            return self.parse_date();
        }
        let start = self.peek_span();
        match self.try_parse_calendar_period()? {
            CalendarPeriod::Once(from, _) => Some(from),
            CalendarPeriod::Yearly(..) => {
                let span = Span::new(start.start, self.last_span.end);
                self.errors.push(Diagnostic::new(
                    span,
                    "`from` needs a year, like `from jul 2027`",
                ));
                None
            }
        }
    }

    /// `<period>` or `<period> through <period>`.
    fn try_parse_window(&mut self) -> Option<Window> {
        let start = self.peek_span();
        let first = self.try_parse_calendar_period()?;
        let last = if self.eat_keyword("through").is_some() {
            let Some(last) = self.try_parse_calendar_period() else {
                let span = self.peek_span();
                self.errors.push(Diagnostic::new(
                    span,
                    "expected a month, quarter or year after `through`",
                ));
                return None;
            };
            last
        } else {
            first
        };
        let span = Span::new(start.start, self.last_span.end);
        match (first, last) {
            (CalendarPeriod::Yearly(first, _), CalendarPeriod::Yearly(_, last)) => {
                Some(Window::Months(month_mask(first, last)))
            }
            (CalendarPeriod::Once(from, _), CalendarPeriod::Once(_, to)) if from < to => {
                Some(Window::Dates(from, Some(to)))
            }
            (CalendarPeriod::Once(..), CalendarPeriod::Once(..)) => {
                self.errors
                    .push(Diagnostic::new(span, "this ends before it starts"));
                None
            }
            _ => {
                self.errors.push(Diagnostic::new(
                    span,
                    "both ends of `through` need a year, or neither",
                ));
                None
            }
        }
    }

    /// A month or quarter, which comes every year, or a year, or a month or
    /// quarter in one.
    fn try_parse_calendar_period(&mut self) -> Option<CalendarPeriod> {
        let (first, last) = if let Some(month) = self.try_parse_month_name() {
            (month as u32, month as u32)
        } else if let Some(quarter) = self.try_parse_quarter_name() {
            (quarter * 3 - 2, quarter * 3)
        } else {
            let year = self.try_parse_full_year()?;
            return Some(CalendarPeriod::Once(
                first_of(year, 1),
                first_of(year + 1, 1),
            ));
        };
        if !matches!(self.peek(), Token::Number(_)) {
            return Some(CalendarPeriod::Yearly(first, last));
        }
        let year = self.try_parse_full_year()?;
        let end = match last {
            12 => first_of(year + 1, 1),
            _ => first_of(year, last + 1),
        };
        Some(CalendarPeriod::Once(first_of(year, first), end))
    }

    fn try_parse_quarter_name(&mut self) -> Option<u32> {
        let Token::Ident(s) = self.peek() else {
            return None;
        };
        let quarter = match *s {
            "q1" => 1,
            "q2" => 2,
            "q3" => 3,
            "q4" => 4,
            _ => return None,
        };
        self.advance();
        Some(quarter)
    }

    /// A year written in full, like `2026`.
    fn try_parse_full_year(&mut self) -> Option<i32> {
        let Token::Number(n) = self.peek() else {
            return None;
        };
        let year = (n.fract().is_zero())
            .then(|| i32::try_from(n.mantissa()).ok())
            .flatten()
            .filter(|y| (1000..=9999).contains(y) && n.scale() == 0);
        let span = self.peek_span();
        self.advance();
        if year.is_none() {
            self.errors.push(Diagnostic::new(
                span,
                "expected a year written in full, like `2026`",
            ));
        }
        year
    }
}

/// What `try_parse_calendar_period` read.
#[derive(Clone, Copy)]
enum CalendarPeriod {
    /// A first and last month, from 1 for January, every year.
    Yearly(u32, u32),
    /// From a first day up to, but not including, a second.
    Once(NaiveDate, NaiveDate),
}

/// The months from `first` through `last`, wrapping past December.
fn month_mask(first: u32, last: u32) -> u16 {
    let mut mask = 0;
    let mut month = first;
    loop {
        mask |= 1 << (month - 1);
        if month == last {
            return mask;
        }
        month = month % 12 + 1;
    }
}

fn first_of(year: i32, month: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(year, month, 1).expect("years are four digits")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Span;

    fn parse_schedule_str(src: &str) -> Result<Schedule, Vec<Diagnostic>> {
        let tokens = crate::lexer::lex(src, 0)?;
        let mut p = Parser::new(tokens);
        let sched = p.parse_schedule_literal();
        if sched.is_none() && p.errors.is_empty() {
            p.errors
                .push(Diagnostic::new(Span::new(0, 0), "expected schedule"));
        }
        if p.errors.is_empty() {
            Ok(sched.unwrap())
        } else {
            Err(p.errors)
        }
    }

    use crate::ast::schedule::*;
    use chrono::NaiveDate;

    fn parse(src: &str) -> Schedule {
        parse_schedule_str(src).unwrap_or_else(|errs| panic!("parse errs: {errs:?}"))
    }

    mod dates {
        use super::*;

        #[test]
        fn parse_single() {
            let sched = parse("2025-01-01");
            let Schedule::Dates(date_list) = sched else {
                panic!("not a date list")
            };
            assert_eq!(1, date_list.len());
            assert_eq!(NaiveDate::from_ymd_opt(2025, 1, 1).unwrap(), date_list[0]);
        }

        #[test]
        fn parse_multiple() {
            let sched = parse("2025-01-01 and 2026-02-12");
            let Schedule::Dates(date_list) = sched else {
                panic!("not a date list")
            };
            assert_eq!(2, date_list.len());
            assert_eq!(NaiveDate::from_ymd_opt(2025, 1, 1).unwrap(), date_list[0]);
            assert_eq!(NaiveDate::from_ymd_opt(2026, 2, 12).unwrap(), date_list[1]);
        }
    }

    mod periodic {
        use super::*;

        mod simple {
            use super::*;

            mod day {
                use super::*;

                #[test]
                fn parses() {
                    let sched = parse("every day");
                    let Schedule::Periodic(periodic) = sched else {
                        panic!("not an every")
                    };
                    assert!(periodic.nth.is_none());
                    assert!(periodic.start.is_none());
                    let Period::Day = periodic.period else {
                        panic!("Not a day")
                    };
                }
            }

            mod week {
                use super::*;

                #[test]
                fn without_on() {
                    let sched = parse("every week");
                    let Schedule::Periodic(periodic) = sched else {
                        panic!("not an every")
                    };
                    assert!(periodic.nth.is_none());
                    assert!(periodic.start.is_none());
                    let Period::Week { on } = periodic.period else {
                        panic!("Not a week")
                    };
                    assert!(on.is_empty());
                }

                #[test]
                fn with_single_on() {
                    let sched = parse("every week on thursday");
                    let Schedule::Periodic(periodic) = sched else {
                        panic!("not an every")
                    };
                    let Period::Week { on } = periodic.period else {
                        panic!("Not a week")
                    };
                    assert_eq!(1, on.len());
                    assert_eq!(Dow::Thursday, on[0]);
                }

                #[test]
                fn with_multiple_on() {
                    let sched = parse("every week on weekend and fri");
                    let Schedule::Periodic(periodic) = sched else {
                        panic!("not an every")
                    };
                    let Period::Week { on } = periodic.period else {
                        panic!("Not a week")
                    };
                    assert_eq!(2, on.len());
                    assert_eq!(Dow::Weekend, on[0]);
                    assert_eq!(Dow::Friday, on[1]);
                }
            }

            mod month {
                use super::*;

                #[test]
                fn named_without_day() {
                    let sched = parse("every july");
                    let Schedule::Periodic(periodic) = sched else {
                        panic!("not an every")
                    };
                    let Period::NamedMonth { month, day } = periodic.period else {
                        panic!("Not a named month")
                    };
                    assert!(day.is_none());
                    assert_eq!(Month::July, month);
                }

                #[test]
                fn named_with_day() {
                    let sched = parse("every aug 1st");
                    let Schedule::Periodic(periodic) = sched else {
                        panic!("not an every")
                    };
                    let Period::NamedMonth { month, day } = periodic.period else {
                        panic!("Not a named month")
                    };
                    assert_eq!(Some(Ordinal::First), day);
                    assert_eq!(Month::August, month);
                }

                #[test]
                fn without_on() {
                    let sched = parse("every month");
                    let Schedule::Periodic(periodic) = sched else {
                        panic!("not an every")
                    };
                    let Period::Month { on } = periodic.period else {
                        panic!("Not a month")
                    };
                    assert!(on.is_empty());
                }

                #[test]
                fn with_ordinal_day() {
                    let sched = parse("every month on the last day");
                    let Schedule::Periodic(periodic) = sched else {
                        panic!("not an every")
                    };
                    let Period::Month { on } = periodic.period else {
                        panic!("Not a month")
                    };
                    assert_eq!(1, on.len());
                    assert_eq!(MonthOccurrence::Day(Ordinal::Last), on[0]);
                }

                #[test]
                fn with_ordinal_weekday() {
                    let sched = parse("every month on the second monday");
                    let Schedule::Periodic(periodic) = sched else {
                        panic!("not an every")
                    };
                    let Period::Month { on } = periodic.period else {
                        panic!("Not a month")
                    };
                    assert_eq!(1, on.len());
                    assert_eq!(
                        MonthOccurrence::Weekday(Ordinal::Nth(Nth::new(2)), Dow::Monday),
                        on[0]
                    );
                }

                #[test]
                fn with_multiple_ons() {
                    let sched = parse("every month on the 3rd thursday, fourth, and 15th weekday");
                    let Schedule::Periodic(periodic) = sched else {
                        panic!("not an every")
                    };
                    let Period::Month { on } = periodic.period else {
                        panic!("Not a month")
                    };
                    assert_eq!(3, on.len());
                    assert_eq!(
                        MonthOccurrence::Weekday(Ordinal::Nth(Nth::new(3)), Dow::Thursday),
                        on[0]
                    );
                    assert_eq!(MonthOccurrence::Day(Ordinal::Nth(Nth::new(4))), on[1]);
                    assert_eq!(
                        MonthOccurrence::Weekday(Ordinal::Nth(Nth::new(15)), Dow::Weekday),
                        on[2]
                    );
                }
            }

            #[test]
            fn quarter() {
                let sched = parse("every quarter");
                let Schedule::Periodic(periodic) = sched else {
                    panic!("not an every")
                };
                assert!(periodic.nth.is_none());
                assert!(periodic.start.is_none());
                let Period::Quarter = periodic.period else {
                    panic!("Not a quarter")
                };
            }

            mod year {
                use super::*;

                #[test]
                fn without_on() {
                    let sched = parse("every year");
                    let Schedule::Periodic(periodic) = sched else {
                        panic!("not an every")
                    };
                    let Period::Year { on } = periodic.period else {
                        panic!("Not a year")
                    };
                    assert!(on.is_empty());
                }

                #[test]
                fn with_single_on() {
                    let sched = parse("every year on april fifth");
                    let Schedule::Periodic(periodic) = sched else {
                        panic!("not an every")
                    };
                    let Period::Year { on } = periodic.period else {
                        panic!("Not a year")
                    };
                    assert_eq!(1, on.len());
                    assert_eq!(Month::April, on[0].0);
                    assert_eq!(Ordinal::Nth(Nth::new(5)), on[0].1);
                }

                #[test]
                fn with_multiple_on() {
                    let sched = parse("every year on may first, jul last");
                    let Schedule::Periodic(periodic) = sched else {
                        panic!("not an every")
                    };
                    let Period::Year { on } = periodic.period else {
                        panic!("Not a year")
                    };
                    assert_eq!(2, on.len());
                    assert_eq!(Month::May, on[0].0);
                    assert_eq!(Ordinal::First, on[0].1);
                    assert_eq!(Month::July, on[1].0);
                    assert_eq!(Ordinal::Last, on[1].1);
                }
            }
        }

        mod adverbial {
            use super::*;

            #[test]
            fn daily() {
                let sched = parse("daily");
                let Schedule::Periodic(periodic) = sched else {
                    panic!("not an every")
                };
                assert!(periodic.nth.is_none());
                assert!(periodic.start.is_none());
                assert_eq!(Period::Day, periodic.period);
            }

            #[test]
            fn weekly_with_on() {
                let sched = parse("weekly on monday and wednesday");
                let Schedule::Periodic(periodic) = sched else {
                    panic!("not an every")
                };
                let Period::Week { on } = periodic.period else {
                    panic!("not a week")
                };
                assert_eq!(vec![Dow::Monday, Dow::Wednesday], on);
            }

            #[test]
            fn monthly_with_on() {
                let sched = parse("monthly on the 1st and last day");
                let Schedule::Periodic(every) = sched else {
                    panic!("not an every")
                };
                let Period::Month { on } = every.period else {
                    panic!("not a month")
                };
                assert_eq!(2, on.len());
                assert_eq!(MonthOccurrence::Day(Ordinal::First), on[0]);
                assert_eq!(MonthOccurrence::Day(Ordinal::Last), on[1]);
            }
        }

        #[test]
        fn rejects_incomplete_lists() {
            let cases = [
                ("weekly on", "expected day of the week"),
                ("weekly on monday and", "expected day of the week"),
                ("monthly on the", "expected day of the month"),
                ("yearly on", "expected month and day"),
                ("2025-01-01,", "expected date"),
            ];
            for (src, expected) in cases {
                let errs = parse_schedule_str(src).unwrap_err();
                assert!(
                    errs.iter().any(|d| d.message.starts_with(expected)),
                    "`{src}`: expected {expected:?}, got {errs:?}"
                );
            }
        }

        #[test]
        fn lists_can_chain_and() {
            let Schedule::Periodic(periodic) = parse("weekly on mon and wed and fri") else {
                panic!("not periodic")
            };
            assert_eq!(
                Period::Week {
                    on: vec![Dow::Monday, Dow::Wednesday, Dow::Friday]
                },
                periodic.period
            );
        }

        #[test]
        fn rejects_impossible_ordinals() {
            let cases = [
                ("monthly on the 32nd", "no month has a 32nd day"),
                ("monthly on the 6th monday", "no month has a 6th Monday"),
                ("monthly on the 24th weekday", "no month has a 24th weekday"),
                ("every feb 30th", "February has at most 29 days"),
                ("yearly on apr 31st", "April has at most 30 days"),
            ];
            for (src, expected) in cases {
                let errs = parse_schedule_str(src).unwrap_err();
                assert!(
                    errs.iter().any(|d| d.message == expected),
                    "`{src}`: expected {expected:?}, got {errs:?}"
                );
            }
            parse("monthly on the 31st, 5th friday, 23rd weekday");
            parse("yearly on feb 29th");
        }

        #[test]
        fn keeps_the_days_in_an_in_list() {
            let Schedule::Periodic(monthly) = parse("monthly on the 1st in jul through apr") else {
                panic!("not periodic")
            };
            assert_eq!(
                When::Windows(vec![Window::Months(0b1111_1100_1111)]),
                monthly.within
            );
            let Schedule::Periodic(every) = parse("every second friday in q2 2027 from 2027-01-01")
            else {
                panic!("not periodic")
            };
            let date = |m| NaiveDate::from_ymd_opt(2027, m, 1).unwrap();
            assert_eq!(
                When::Windows(vec![Window::Dates(date(4), Some(date(7)))]),
                every.within
            );
            assert_eq!(Some(date(1)), every.start);
            let Schedule::Periodic(daily) = parse("daily") else {
                panic!("not periodic")
            };
            assert_eq!(When::Always, daily.within);
        }

        #[test]
        fn bare_numbers_are_days() {
            let Schedule::Periodic(periodic) = parse("yearly on jan 1, jul 31") else {
                panic!("not periodic")
            };
            assert_eq!(
                Period::Year {
                    on: vec![
                        (Month::January, Ordinal::First),
                        (Month::July, Ordinal::Nth(Nth::new(31))),
                    ]
                },
                periodic.period
            );
            let Schedule::Periodic(periodic) = parse("monthly on the 1") else {
                panic!("not periodic")
            };
            assert_eq!(
                Period::Month {
                    on: vec![MonthOccurrence::Day(Ordinal::First)]
                },
                periodic.period
            );
        }

        #[test]
        fn numeric_plural_form() {
            let sched = parse("every 2 fridays from 2025-01-01");
            let Schedule::Periodic(every) = sched else {
                panic!("not an every")
            };
            assert_eq!(Some(Nth::new(2)), every.nth);
            assert_eq!(
                Some(NaiveDate::from_ymd_opt(2025, 1, 1).unwrap()),
                every.start
            );
            assert_eq!(Period::Weekday(Dow::Friday), every.period);
        }

        #[test]
        fn compund() {
            let sched = parse("every second friday from 2025-01-01");
            let Schedule::Periodic(every) = sched else {
                panic!("not an every")
            };
            let Some(nth) = every.nth else {
                panic!("nth is missing")
            };
            let Some(start) = every.start else {
                panic!("start is missing")
            };
            assert_eq!(Nth::new(2), nth);
            assert_eq!(Period::Weekday(Dow::Friday), every.period);
            assert_eq!(NaiveDate::from_ymd_opt(2025, 1, 1).unwrap(), start);
        }
    }
}
