use crate::ast::schedule::{Dow, Every, Month, MonthOccurrence, Ordinal, Period, Schedule};
use crate::{lexer::Token, Span};

use chrono::NaiveDate;
use chumsky::{input::ValueInput, prelude::*};

pub fn parse_schedules<'src, I>(
) -> impl Parser<'src, I, Schedule, extra::Err<Rich<'src, Token<'src>, Span>>> + Clone
where
    I: ValueInput<'src, Token = Token<'src>, Span = Span>,
{
    // <date> ::= "2" "0" [0-9] [0-9] "-" ("0" [1-9] | "1" [0-2]) "-" (([0-2] [1-9]) | ("3" [0-1]))
    let date = select! { Token::Date(d) => d }.labelled("date");

    // <date_list> ::= <date> ((", " <date>)* ((", and " | " and ") <date>))?
    let date_list = date
        .clone()
        .separated_by(just(Token::Comma))
        .at_least(1)
        .collect::<Vec<NaiveDate>>();

    // <nth> ::= "2nd" | "3rd" | [4-9] "th" | "second" | "third" | "fourth" | "fifth" | "sixth" | "seventh" | "eighth" | "ninth"
    let nth = choice((
        just(Token::Ident("2nd")).to(Ordinal::Nth(2)),
        just(Token::Ident("second")).to(Ordinal::Nth(2)),
        just(Token::Ident("3rd")).to(Ordinal::Nth(3)),
        just(Token::Ident("third")).to(Ordinal::Nth(3)),
        just(Token::Ident("4th")).to(Ordinal::Nth(4)),
        just(Token::Ident("fourth")).to(Ordinal::Nth(4)),
        just(Token::Ident("5th")).to(Ordinal::Nth(4)),
        just(Token::Ident("fifth")).to(Ordinal::Nth(5)),
        just(Token::Ident("6th")).to(Ordinal::Nth(6)),
        just(Token::Ident("sixth")).to(Ordinal::Nth(6)),
        just(Token::Ident("7th")).to(Ordinal::Nth(7)),
        just(Token::Ident("seventh")).to(Ordinal::Nth(7)),
        just(Token::Ident("8th")).to(Ordinal::Nth(8)),
        just(Token::Ident("eight")).to(Ordinal::Nth(8)),
        just(Token::Ident("9th")).to(Ordinal::Nth(9)),
        just(Token::Ident("ninth")).to(Ordinal::Nth(9)),
        just(Token::Ident("10th")).to(Ordinal::Nth(10)),
        just(Token::Ident("tenth")).to(Ordinal::Nth(10)),
    ));

    // <ordinal> ::= "first" | "1st" | <nth> | "last"
    let ordinal = choice((
        just(Token::Ident("1st"))
            .or(just(Token::Ident("first")))
            .to(Ordinal::Nth(1)),
        nth.clone(),
        just(Token::Ident("last")).to(Ordinal::Last),
    ));

    // <month_name> ::= "january" | "february" | "march" | "april" | "may" | "june" | "july" | "august" | "september" | "october" | "november" | "december"
    let month_name = choice((
        just(Token::Ident("january")).to(Month::January),
        just(Token::Ident("jan")).to(Month::January),
        just(Token::Ident("february")).to(Month::February),
        just(Token::Ident("feb")).to(Month::February),
        just(Token::Ident("march")).to(Month::March),
        just(Token::Ident("mar")).to(Month::March),
        just(Token::Ident("april")).to(Month::April),
        just(Token::Ident("apr")).to(Month::April),
        just(Token::Ident("may")).to(Month::May),
        just(Token::Ident("june")).to(Month::June),
        just(Token::Ident("jun")).to(Month::June),
        just(Token::Ident("july")).to(Month::July),
        just(Token::Ident("jul")).to(Month::July),
        just(Token::Ident("august")).to(Month::August),
        just(Token::Ident("aug")).to(Month::August),
        just(Token::Ident("september")).to(Month::September),
        just(Token::Ident("sep")).to(Month::September),
        just(Token::Ident("october")).to(Month::October),
        just(Token::Ident("oct")).to(Month::October),
        just(Token::Ident("november")).to(Month::November),
        just(Token::Ident("nov")).to(Month::November),
        just(Token::Ident("december")).to(Month::December),
        just(Token::Ident("dec")).to(Month::December),
    ));

    // <dow> ::= "monday" | "tuesday" | "wednesday" | "thursday" | "friday" | "saturday" | "sunday" | "weekday" | "weekend day"
    let dow = choice((
        just(Token::Ident("Monday")).to(Dow::Monday),
        just(Token::Ident("monday")).to(Dow::Monday),
        just(Token::Ident("mon")).to(Dow::Monday),
        just(Token::Ident("Tuesday")).to(Dow::Tuesday),
        just(Token::Ident("tuesday")).to(Dow::Tuesday),
        just(Token::Ident("tue")).to(Dow::Tuesday),
        just(Token::Ident("Wednesday")).to(Dow::Wednesday),
        just(Token::Ident("wednesday")).to(Dow::Wednesday),
        just(Token::Ident("wed")).to(Dow::Wednesday),
        just(Token::Ident("Thursday")).to(Dow::Thursday),
        just(Token::Ident("thursday")).to(Dow::Thursday),
        just(Token::Ident("thu")).to(Dow::Thursday),
        just(Token::Ident("Friday")).to(Dow::Friday),
        just(Token::Ident("friday")).to(Dow::Friday),
        just(Token::Ident("fri")).to(Dow::Friday),
        just(Token::Ident("Saturday")).to(Dow::Saturday),
        just(Token::Ident("saturday")).to(Dow::Saturday),
        just(Token::Ident("sat")).to(Dow::Saturday),
        just(Token::Ident("Sunday")).to(Dow::Sunday),
        just(Token::Ident("sunday")).to(Dow::Sunday),
        just(Token::Ident("sun")).to(Dow::Sunday),
        just(Token::Ident("Weekday")).to(Dow::Weekday),
        just(Token::Ident("weekday")).to(Dow::Weekday),
        just(Token::Ident("Weekend"))
            .then(just(Token::Ident("Day")))
            .to(Dow::WeekendDay),
        just(Token::Ident("weekend"))
            .then(just(Token::Ident("day")))
            .to(Dow::WeekendDay),
    ));

    // <ordinal_day> ::= <ordinal> " day"?
    let ordinal_day = ordinal
        .clone()
        .then_ignore(just(Token::Ident("day")).or_not());

    // <ordinal_weekday> ::= <ordinal> " " <dow>
    let ordinal_weekday = ordinal.clone().then(dow.clone());

    // <month_occurrence> ::= <ordinal_day> | <ordinal_weekday>
    let month_occurrence = choice((
        ordinal_weekday
            .clone()
            .map(|(ordinal, dow)| MonthOccurrence::Weekday(ordinal, dow)),
        ordinal_day.clone().map(MonthOccurrence::Day),
    ));

    // <month_occurrence_list> ::= <month_occurrence> ((", " <month_occurrence>)* ((", and " | " and ") <month_occurrence>))?
    let month_occurrence_list = month_occurrence
        .clone()
        .separated_by(just(Token::Comma))
        .at_least(1)
        .collect::<Vec<MonthOccurrence>>();

    // <start> ::= " starting on " <date>
    let start = just(Token::Ident("starting"))
        .ignore_then(just(Token::Ident("on")))
        .ignore_then(date);

    // <dow_list> ::= <dow> ((", " <dow>)* ((", and " | " and ") <dow>))?
    // TODO: parse (AND)
    let dow_list = dow
        .clone()
        .separated_by(just(Token::Comma))
        .at_least(1)
        .collect::<Vec<Dow>>();

    // <day> ::= "day"
    let day = just(Token::Ident("day")).to(Period::Day);

    // <week> ::= "week" (" on " <dow_list>)?
    let week = just(Token::Ident("week"))
        .ignore_then(just(Token::Ident("on")).ignore_then(dow_list).or_not())
        .map(|dows| {
            let on = dows.unwrap_or_else(Vec::new);
            Period::Week { on }
        });

    // <month> ::= <month_name> (" " <ordinal>)? | "month" (" on the " <month_occurrence_list>)?
    let month = choice((
        month_name
            .clone()
            .then(ordinal.clone().or_not())
            .map(|(month, day)| Period::NamedMonth { month, day }),
        just(Token::Ident("month"))
            .ignore_then(
                just(Token::Ident("on"))
                    .ignore_then(just(Token::Ident("the")))
                    .ignore_then(month_occurrence_list)
                    .or_not(),
            )
            .map(|on| Period::Month {
                on: on.unwrap_or_else(Vec::new),
            }),
    ));

    // <quarter> ::= "quarter"
    let quarter = just(Token::Ident("quarter")).to(Period::Quarter);

    // <year> ::= "year" (" on " <month_name> <ordinal>)?
    let year = just(Token::Ident("year"))
        .ignore_then(
            just(Token::Ident("on"))
                .ignore_then(month_name)
                .then(ordinal.clone())
                .or_not(),
        )
        .map(|on| Period::Year { on });

    // <period> ::= "day" | <week> | <dow> | <month> | <quarter> | <year>
    let period = choice((
        day,
        dow.clone().map(Period::Weekday),
        week,
        month,
        quarter,
        year,
    ));

    // <simple_every> ::= "every " <period>
    let simple_every = just(Token::Ident("every"))
        .ignore_then(period.clone())
        .map(|period| Every {
            nth: None,
            period,
            start: None,
        });

    // <anchored_every> ::= "every " (<nth> " ")? <period> <start>
    let anchored_every = just(Token::Ident("every"))
        .ignore_then(nth.or_not())
        .then(period)
        .then(start)
        .map(|((nth, period), start)| Every {
            nth,
            period,
            start: Some(start),
        });

    // <every> ::= <simple_every> | <anchored_every>
    let every = choice((simple_every, anchored_every)).map(Schedule::Every);

    // <shifter> ::= <ordinal> " " <dow> " on or"? (" before " | " after ")
    let _shifter = ordinal
        .then(dow)
        .then_ignore(just(Token::Ident("on")).then(just(Token::Ident("or"))))
        .or_not()
        .then(just(Token::Ident("before")).or(just(Token::Ident("after"))));

    // <schedule> ::= <shifter>? (<every> | <date_list>)
    every.or(date_list.map(Schedule::Dates))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lexer::lexer;
    use chrono::NaiveDate;
    use chumsky::Parser;

    fn parse(src: &str) -> Schedule {
        let (tokens, lex_errs) = lexer().parse(src).into_output_errors();
        assert!(lex_errs.is_empty(), "lex errs: {lex_errs:?}");
        let tokens = tokens.unwrap();
        let eoi = (src.len()..src.len()).into();
        let input = tokens.as_slice().map(eoi, |(t, s)| (t, s));
        let (sched, errs) = parse_schedules().parse(input).into_output_errors();
        assert!(errs.is_empty(), "parse errs: {errs:?}");
        sched.unwrap()
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
            let sched = parse("2025-01-01, 2026-02-12");
            let Schedule::Dates(date_list) = sched else {
                panic!("not a date list")
            };
            assert_eq!(2, date_list.len());
            assert_eq!(NaiveDate::from_ymd_opt(2025, 1, 1).unwrap(), date_list[0]);
            assert_eq!(NaiveDate::from_ymd_opt(2026, 2, 12).unwrap(), date_list[1]);
        }
    }

    mod every {
        use super::*;

        mod simple {
            use super::*;

            mod day {
                use super::*;
                #[test]
                fn parses() {
                    let sched = parse("every day");
                    let Schedule::Every(every) = sched else {
                        panic!("not an every")
                    };
                    assert!(every.nth.is_none());
                    assert!(every.start.is_none());

                    let Period::Day = every.period else {
                        panic!("Not a year")
                    };
                }
            }

            mod week {
                use super::*;

                #[test]
                fn without_on() {
                    let sched = parse("every week");
                    let Schedule::Every(every) = sched else {
                        panic!("not an every")
                    };
                    assert!(every.nth.is_none());
                    assert!(every.start.is_none());

                    let Period::Week { on } = every.period else {
                        panic!("Not a week")
                    };
                    assert!(on.is_empty());
                }

                #[test]
                fn with_single_on() {
                    let sched = parse("every week on thursday");
                    let Schedule::Every(every) = sched else {
                        panic!("not an every")
                    };
                    assert!(every.nth.is_none());
                    assert!(every.start.is_none());

                    let Period::Week { on } = every.period else {
                        panic!("Not a week")
                    };
                    assert_eq!(1, on.len());
                    assert_eq!(Dow::Thursday, on[0]);
                }

                #[test]
                fn with_multiple_on() {
                    let sched = parse("every week on weekend day, fri");
                    let Schedule::Every(every) = sched else {
                        panic!("not an every")
                    };
                    assert!(every.nth.is_none());
                    assert!(every.start.is_none());

                    let Period::Week { on } = every.period else {
                        panic!("Not a week")
                    };
                    assert_eq!(2, on.len());
                    assert_eq!(Dow::WeekendDay, on[0]);
                    assert_eq!(Dow::Friday, on[1]);
                }
            }

            mod month {
                use super::*;

                #[test]
                fn named_without_day() {
                    let sched = parse("every july");
                    let Schedule::Every(every) = sched else {
                        panic!("not an every")
                    };
                    assert!(every.nth.is_none());
                    assert!(every.start.is_none());

                    let Period::NamedMonth { month, day } = every.period else {
                        panic!("Not a named month")
                    };
                    assert!(day.is_none());
                    assert_eq!(Month::July, month);
                }

                #[test]
                fn named_with_day() {
                    let sched = parse("every aug first");
                    let Schedule::Every(every) = sched else {
                        panic!("not an every")
                    };
                    assert!(every.nth.is_none());
                    assert!(every.start.is_none());

                    let Period::NamedMonth { month, day } = every.period else {
                        panic!("Not a named month")
                    };
                    assert_eq!(Some(Ordinal::Nth(1)), day);
                    assert_eq!(Month::August, month);
                }

                #[test]
                fn without_on() {
                    let sched = parse("every month");
                    let Schedule::Every(every) = sched else {
                        panic!("not an every")
                    };
                    assert!(every.nth.is_none());
                    assert!(every.start.is_none());

                    let Period::Month { on } = every.period else {
                        panic!("Not a month")
                    };
                    assert!(on.is_empty());
                }

                #[test]
                fn with_ordinal_day() {
                    let sched = parse("every month on the last day");
                    let Schedule::Every(every) = sched else {
                        panic!("not an every")
                    };
                    assert!(every.nth.is_none());
                    assert!(every.start.is_none());

                    let Period::Month { on } = every.period else {
                        panic!("Not a month")
                    };
                    assert_eq!(1, on.len());
                    assert_eq!(MonthOccurrence::Day(Ordinal::Last), on[0]);
                }

                #[test]
                fn with_ordinal_weekday() {
                    let sched = parse("every month on the second monday");
                    let Schedule::Every(every) = sched else {
                        panic!("not an every")
                    };
                    assert!(every.nth.is_none());
                    assert!(every.start.is_none());

                    let Period::Month { on } = every.period else {
                        panic!("Not a month")
                    };
                    assert_eq!(1, on.len());
                    assert_eq!(
                        MonthOccurrence::Weekday(Ordinal::Nth(2), Dow::Monday),
                        on[0]
                    );
                }

                #[test]
                fn with_multiple_ons() {
                    let sched = parse("every month on the third thursday, fourth, fifth weekday");
                    let Schedule::Every(every) = sched else {
                        panic!("not an every")
                    };
                    assert!(every.nth.is_none());
                    assert!(every.start.is_none());

                    let Period::Month { on } = every.period else {
                        panic!("Not a month")
                    };
                    assert_eq!(3, on.len());
                    assert_eq!(
                        MonthOccurrence::Weekday(Ordinal::Nth(3), Dow::Thursday),
                        on[0]
                    );
                    assert_eq!(MonthOccurrence::Day(Ordinal::Nth(4)), on[1]);
                    assert_eq!(
                        MonthOccurrence::Weekday(Ordinal::Nth(5), Dow::Weekday),
                        on[2]
                    );
                }
            }

            #[test]
            fn quarter() {
                let sched = parse("every quarter");
                let Schedule::Every(every) = sched else {
                    panic!("not an every")
                };
                assert!(every.nth.is_none());
                assert!(every.start.is_none());

                let Period::Quarter = every.period else {
                    panic!("Not a quarter")
                };
            }

            mod year {
                use super::*;

                #[test]
                fn without_on() {
                    let sched = parse("every year");
                    let Schedule::Every(every) = sched else {
                        panic!("not an every")
                    };
                    assert!(every.nth.is_none());
                    assert!(every.start.is_none());

                    let Period::Year { on } = every.period else {
                        panic!("Not a year")
                    };
                    assert!(on.is_none());
                }

                #[test]
                fn with_on() {
                    let sched = parse("every year on april fifth");
                    let Schedule::Every(every) = sched else {
                        panic!("not an every")
                    };
                    assert!(every.nth.is_none());
                    assert!(every.start.is_none());

                    let Period::Year { on } = every.period else {
                        panic!("Not a year")
                    };

                    let Some((month, ordinal)) = on else {
                        panic!("On not present");
                    };

                    assert_eq!(Month::April, month);
                    assert_eq!(Ordinal::Nth(5), ordinal);
                }
            }
        }

        #[test]
        fn compund() {
            let sched = parse("every second friday starting on 2025-01-01");
            let Schedule::Every(every) = sched else {
                panic!("not an every")
            };
            let Some(nth) = every.nth else {
                panic!("nth is missing")
            };
            let Some(start) = every.start else {
                panic!("start is missing")
            };
            assert_eq!(Ordinal::Nth(2), nth);
            assert_eq!(Period::Weekday(Dow::Friday), every.period);
            assert_eq!(NaiveDate::from_ymd_opt(2025, 1, 1).unwrap(), start);
        }
    }
}
