use super::util::{comma_list, ident_ci};
use crate::ast::schedule::{Dow, Every, Month, MonthOccurrence, Ordinal, Period, Schedule};
use crate::{lexer::Token, Span};

use chumsky::{input::ValueInput, prelude::*};
use rust_decimal::Decimal;

pub fn parse_schedule<'src, I>(
) -> impl Parser<'src, I, Schedule, extra::Err<Rich<'src, Token<'src>, Span>>> + Clone
where
    I: ValueInput<'src, Token = Token<'src>, Span = Span>,
{
    // <date> ::= [0-9] [0-9] [0-9] [0-9] "-" ("0" [1-9] | "1" [0-2]) "-" (([0-2] [1-9]) | ("3" [0-1]))
    let date = select! { Token::Date(d) => d }.labelled("date");

    // <date_list> ::= <date> ((", " <date>)* ((", and " | " and ") <date>))?
    let date_list = comma_list(date);

    // <nth> ::= "2nd" | "3rd" | [4-9] "th" | "second" | "third" | "fourth" | "fifth" | "sixth" | "seventh" | "eighth" | "ninth"
    let nth = choice((
        select! { Token::Ordinal(n) if n >= 2 => Ordinal::Nth(n) },
        select! { Token::Float(n)
            if n.fract() == Decimal::ZERO && n >= Decimal::from(2) && n <= Decimal::from(255)
            => Ordinal::Nth(n.to_string().parse().unwrap())
        },
        ident_ci("second").to(Ordinal::Nth(2)),
        ident_ci("third").to(Ordinal::Nth(3)),
        ident_ci("fourth").to(Ordinal::Nth(4)),
        ident_ci("fifth").to(Ordinal::Nth(5)),
        ident_ci("sixth").to(Ordinal::Nth(6)),
        ident_ci("seventh").to(Ordinal::Nth(7)),
        ident_ci("eighth").to(Ordinal::Nth(8)),
        ident_ci("ninth").to(Ordinal::Nth(9)),
        ident_ci("tenth").to(Ordinal::Nth(10)),
    ));

    // <ordinal> ::= "first" | "1st" | <nth> | "last"
    let ordinal = choice((
        just(Token::Ordinal(1))
            .or(just(Token::Ident("first")))
            .to(Ordinal::Nth(1)),
        nth.clone(),
        ident_ci("last").to(Ordinal::Last),
    ));

    // <month_name> ::= "january" | "february" | "march" | "april" | "may" | "june" | "july" | "august" | "september" | "october" | "november" | "december"
    let month_name = choice((
        ident_ci("january").or(ident_ci("jan")).to(Month::January),
        ident_ci("february").or(ident_ci("feb")).to(Month::February),
        ident_ci("march").or(ident_ci("mar")).to(Month::March),
        ident_ci("april").or(ident_ci("apr")).to(Month::April),
        ident_ci("may").to(Month::May),
        ident_ci("june").or(ident_ci("jun")).to(Month::June),
        ident_ci("july").or(ident_ci("jul")).to(Month::July),
        ident_ci("august").or(ident_ci("aug")).to(Month::August),
        ident_ci("september")
            .or(ident_ci("sep"))
            .to(Month::September),
        ident_ci("october").or(ident_ci("oct")).to(Month::October),
        ident_ci("november").or(ident_ci("nov")).to(Month::November),
        ident_ci("december").or(ident_ci("dec")).to(Month::December),
    ));

    // <dow> ::= "monday" | "tuesday" | "wednesday" | "thursday" | "friday" | "saturday" | "sunday" | "weekday" | "weekend day"
    let dow = choice((
        ident_ci("monday").or(ident_ci("mondays")).or(ident_ci("mon")).to(Dow::Monday),
        ident_ci("tuesday").or(ident_ci("tuesdays")).or(ident_ci("tue")).to(Dow::Tuesday),
        ident_ci("wednesday").or(ident_ci("wednesdays")).or(ident_ci("wed")).to(Dow::Wednesday),
        ident_ci("thursday").or(ident_ci("thursdays")).or(ident_ci("thu")).to(Dow::Thursday),
        ident_ci("friday").or(ident_ci("fridays")).or(ident_ci("fri")).to(Dow::Friday),
        ident_ci("saturday").or(ident_ci("saturdays")).or(ident_ci("sat")).to(Dow::Saturday),
        ident_ci("sunday").or(ident_ci("sundays")).or(ident_ci("sun")).to(Dow::Sunday),
        ident_ci("weekday").or(ident_ci("weekdays")).to(Dow::Weekday),
        ident_ci("weekend")
            .then(ident_ci("days").or(ident_ci("day")).or_not())
            .to(Dow::WeekendDay),
    ));

    // <ordinal_day> ::= <ordinal> " day"?
    let ordinal_day = ordinal.clone().then_ignore(ident_ci("day").or_not());

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
    let month_occurrence_list = comma_list(month_occurrence);

    // <start> ::= " starting on " <date>
    let start = ident_ci("starting")
        .ignore_then(ident_ci("on"))
        .ignore_then(date);

    // <dow_list> ::= <dow> ((", " <dow>)* ((", and " | " and ") <dow>))?
    let dow_list = comma_list(dow.clone());

    // <day> ::= "day"
    let day = ident_ci("day").or(ident_ci("days")).to(Period::Day);

    // <week> ::= "week" (" on " <dow_list>)?
    let week = ident_ci("week")
        .or(ident_ci("weeks"))
        .ignore_then(ident_ci("on").ignore_then(dow_list.clone()).or_not())
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
        ident_ci("month")
            .or(ident_ci("months"))
            .ignore_then(
                ident_ci("on")
                    .ignore_then(ident_ci("the"))
                    .ignore_then(month_occurrence_list.clone())
                    .or_not(),
            )
            .map(|on| Period::Month {
                on: on.unwrap_or_else(Vec::new),
            }),
    ));

    // <quarter> ::= "quarter"
    let quarter = ident_ci("quarter").or(ident_ci("quarters")).to(Period::Quarter);

    // <year> ::= "year" (" on " <month_name> <ordinal>)?
    let year = ident_ci("year")
        .or(ident_ci("years"))
        .ignore_then(
            ident_ci("on")
                .ignore_then(month_name.clone())
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
    let simple_every = ident_ci("every")
        .ignore_then(period.clone())
        .map(|period| Every {
            nth: None,
            period,
            start: None,
        });

    // <anchored_every> ::= "every " (<nth> " ")? <period> <start>
    let anchored_every = ident_ci("every")
        .ignore_then(nth.or_not())
        .then(period)
        .then(start)
        .map(|((nth, period), start)| Every {
            nth,
            period,
            start: Some(start),
        });

    // daily | weekly [on <dow_list>] | monthly [on the <month_occurrence_list>] | quarterly | yearly/annually [on <month> <ordinal>]
    let adverbial = choice((
        ident_ci("daily").to(Period::Day),
        ident_ci("weekly")
            .ignore_then(ident_ci("on").ignore_then(dow_list).or_not())
            .map(|dows| Period::Week { on: dows.unwrap_or_default() }),
        ident_ci("monthly")
            .ignore_then(
                ident_ci("on")
                    .ignore_then(ident_ci("the"))
                    .ignore_then(month_occurrence_list)
                    .or_not(),
            )
            .map(|on| Period::Month { on: on.unwrap_or_default() }),
        ident_ci("quarterly").to(Period::Quarter),
        ident_ci("yearly")
            .or(ident_ci("annually"))
            .ignore_then(
                ident_ci("on")
                    .ignore_then(month_name)
                    .then(ordinal.clone())
                    .or_not(),
            )
            .map(|on| Period::Year { on }),
    ))
    .map(|period| Every { nth: None, period, start: None });

    // <every> ::= <simple_every> | <anchored_every> | <adverbial>
    let every = choice((simple_every, anchored_every, adverbial)).map(Schedule::Every);

    // <shifter> ::= <ordinal> " " <dow> " on or"? (" before " | " after ")
    let _shifter = ordinal
        .then(dow)
        .then_ignore(ident_ci("on").then(ident_ci("or")).or_not())
        .then(ident_ci("before").or(ident_ci("after")));

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
        let (sched, errs) = parse_schedule().parse(input).into_output_errors();
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
            let sched = parse("2025-01-01 and 2026-02-12");
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
                    let sched = parse("every week on weekend day and fri");
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
                    let sched = parse("every aug 1st");
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
                    let sched = parse("every month on the 3rd thursday, fourth, and 15th weekday");
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
                        MonthOccurrence::Weekday(Ordinal::Nth(15), Dow::Weekday),
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

        mod adverbial {
            use super::*;

            #[test]
            fn daily() {
                let sched = parse("daily");
                let Schedule::Every(every) = sched else { panic!("not an every") };
                assert!(every.nth.is_none());
                assert!(every.start.is_none());
                assert_eq!(Period::Day, every.period);
            }

            #[test]
            fn weekly_with_on() {
                let sched = parse("weekly on monday and wednesday");
                let Schedule::Every(every) = sched else { panic!("not an every") };
                let Period::Week { on } = every.period else { panic!("not a week") };
                assert_eq!(vec![Dow::Monday, Dow::Wednesday], on);
            }

            #[test]
            fn monthly_with_on() {
                let sched = parse("monthly on the 1st and last day");
                let Schedule::Every(every) = sched else { panic!("not an every") };
                let Period::Month { on } = every.period else { panic!("not a month") };
                assert_eq!(2, on.len());
                assert_eq!(MonthOccurrence::Day(Ordinal::Nth(1)), on[0]);
                assert_eq!(MonthOccurrence::Day(Ordinal::Last), on[1]);
            }
        }

        #[test]
        fn numeric_plural_form() {
            let sched = parse("every 2 fridays starting on 2025-01-01");
            let Schedule::Every(every) = sched else {
                panic!("not an every")
            };
            assert_eq!(Some(Ordinal::Nth(2)), every.nth);
            assert_eq!(
                Some(NaiveDate::from_ymd_opt(2025, 1, 1).unwrap()),
                every.start
            );
            assert_eq!(Period::Weekday(Dow::Friday), every.period);
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
