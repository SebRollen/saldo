use super::Parser;
use crate::ast::schedule::{Dow, Month, MonthOccurrence, Nth, Ordinal, Period, Periodic, Schedule};
use crate::errors::Diagnostic;
use crate::lexer::Token;
use chrono::NaiveDate;
use rust_decimal::Decimal;

impl<'src> Parser<'src> {
    pub(super) fn parse_schedule_literal(&mut self) -> Option<Schedule> {
        match self.peek() {
            Token::Ident(s) => {
                let s = *s;
                if s.eq_ignore_ascii_case("every") {
                    self.parse_periodic()
                } else if matches!(
                    s.to_lowercase().as_str(),
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
        self.eat_ident_ci("every")?;
        let nth = self.try_parse_nth();
        let period = self.parse_period()?;
        let start = if self.eat_ident_ci("from").is_some() {
            Some(self.parse_date()?)
        } else {
            None
        };
        Some(Schedule::Periodic(Periodic { nth, period, start }))
    }

    fn parse_adverbial(&mut self) -> Option<Schedule> {
        let Token::Ident(s) = self.peek() else {
            return None;
        };
        let s = s.to_lowercase();
        let period = match s.as_str() {
            "daily" => {
                self.advance();
                Period::Day
            }
            "weekly" => {
                self.advance();
                let on = if self.eat_ident_ci("on").is_some() {
                    self.parse_dow_list()
                } else {
                    Vec::new()
                };
                Period::Week { on }
            }
            "monthly" => {
                self.advance();
                let on = if self.eat_ident_ci("on").is_some() {
                    self.eat_ident_ci("the");
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
                let on = if self.eat_ident_ci("on").is_some() {
                    self.parse_year_occurrence_list()
                } else {
                    Vec::new()
                };
                Period::Year { on }
            }
            _ => return None,
        };
        Some(Schedule::Periodic(Periodic {
            nth: None,
            period,
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
            && (s.eq_ignore_ascii_case("day") || s.eq_ignore_ascii_case("days")) {
                self.advance();
                return Some(Period::Day);
        }
        None
    }

    fn try_parse_week(&mut self) -> Option<Period> {
        if let Token::Ident(s) = self.peek()
            && (s.eq_ignore_ascii_case("week") || s.eq_ignore_ascii_case("weeks")) {
                self.advance();
                let on = if self.eat_ident_ci("on").is_some() {
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
            let day = self.try_parse_ordinal();
            return Some(Period::NamedMonth { month, day });
        }
        if let Token::Ident(s) = self.peek()
            && (s.eq_ignore_ascii_case("month") || s.eq_ignore_ascii_case("months")) {
                self.advance();
                let on = if self.eat_ident_ci("on").is_some() {
                    self.eat_ident_ci("the");
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
            && (s.eq_ignore_ascii_case("quarter") || s.eq_ignore_ascii_case("quarters")) {
                self.advance();
                return Some(Period::Quarter);
        }
        None
    }

    fn try_parse_year(&mut self) -> Option<Period> {
        if let Token::Ident(s) = self.peek() 
            && (s.eq_ignore_ascii_case("year") || s.eq_ignore_ascii_case("years")) {
                self.advance();
                let on = if self.eat_ident_ci("on").is_some() {
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
        let dow = match s.to_lowercase().as_str() {
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
        let month = match s.to_lowercase().as_str() {
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
        if let Token::Ordinal(n) = self.peek() && *n >= 2 {
            let n = *n;
            self.advance();
            return Some(Nth::new(n));
        }
        if let Token::Float(n) = self.peek() {
            let n = *n;
            if n.fract() == Decimal::ZERO && n >= Decimal::from(2) && n <= Decimal::from(255)
                && let Ok(val) = n.to_string().parse::<u8>() {
                    self.advance();
                    return Some(Nth::new(val));
            }
        }
        if let Token::Ident(s) = self.peek() {
            let n: u8 = match s.to_lowercase().as_str() {
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
        if let Token::Ident(s) = self.peek() {
            if s.eq_ignore_ascii_case("first") {
                self.advance();
                return Some(Ordinal::First);
            }
            if s.eq_ignore_ascii_case("last") {
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
        self.parse_comma_list(|p| p.try_parse_dow())
    }

    fn parse_month_occurrence_list(&mut self) -> Vec<MonthOccurrence> {
        self.parse_comma_list(|p| p.try_parse_month_occurrence())
    }

    fn try_parse_month_occurrence(&mut self) -> Option<MonthOccurrence> {
        let ordinal = self.try_parse_ordinal()?;
        if let Some(dow) = self.try_parse_dow() {
            return Some(MonthOccurrence::Weekday(ordinal, dow));
        }
        let _ = self
            .eat_ident_ci("day")
            .or_else(|| self.eat_ident_ci("days"));
        Some(MonthOccurrence::Day(ordinal))
    }

    fn parse_year_occurrence_list(&mut self) -> Vec<(Month, Ordinal)> {
        self.parse_comma_list(|p| p.try_parse_year_occurrence())
    }

    fn try_parse_year_occurrence(&mut self) -> Option<(Month, Ordinal)> {
        let month = self.try_parse_month_name()?;
        let ordinal = self.require_ordinal()?;
        Some((month, ordinal))
    }

    fn parse_date_list(&mut self) -> Option<Vec<NaiveDate>> {
        if !matches!(self.peek(), Token::Date(_)) {
            return None;
        }
        Some(self.parse_comma_list(|p| {
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Lexer, Span};

    fn parse_schedule_str(src: &str) -> Result<Schedule, Vec<Diagnostic>> {
        let tokens = Lexer::new(src).lex()?;
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
