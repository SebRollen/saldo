use chrono::{Datelike, NaiveDate};

#[derive(Debug, Clone, PartialEq)]
pub enum Dow {
    Monday,
    Tuesday,
    Wednesday,
    Thursday,
    Friday,
    Saturday,
    Sunday,
    Weekday,
    Weekend,
}

impl Dow {
    fn matches(&self, t: NaiveDate) -> bool {
        use chrono::Weekday as W;
        match self {
            Dow::Monday => t.weekday() == W::Mon,
            Dow::Tuesday => t.weekday() == W::Tue,
            Dow::Wednesday => t.weekday() == W::Wed,
            Dow::Thursday => t.weekday() == W::Thu,
            Dow::Friday => t.weekday() == W::Fri,
            Dow::Saturday => t.weekday() == W::Sat,
            Dow::Sunday => t.weekday() == W::Sun,
            Dow::Weekday => matches!(t.weekday(), W::Mon | W::Tue | W::Wed | W::Thu | W::Fri),
            Dow::Weekend => matches!(t.weekday(), W::Sat | W::Sun),
        }
    }
}

#[derive(Debug, Copy, Clone, PartialEq)]
pub enum Month {
    January = 1,
    February = 2,
    March = 3,
    April = 4,
    May = 5,
    June = 6,
    July = 7,
    August = 8,
    September = 9,
    October = 10,
    November = 11,
    December = 12,
}

impl Month {
    pub fn matches(&self, t: NaiveDate) -> bool {
        *self as u32 == t.month()
    }
}

// Non-first ordinal
#[derive(Debug, Clone, PartialEq)]
pub struct Nth(u8);

impl Nth {
    pub fn new(inner: u8) -> Self{
        assert!(inner > 1, "Nth must be > 1; use Ordinal::First for 1");
        Self(inner)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Ordinal {
    First,
    Nth(Nth),
    Last,
}

impl Ordinal {
    pub fn matches(&self, t: NaiveDate) -> bool {
        match self {
            Ordinal::First => t.day() == 1,
            Ordinal::Nth(Nth(n)) => t.day() == (*n).into(),
            Ordinal::Last => t.is_month_end(),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum MonthOccurrence {
    Day(Ordinal),          // "1st [day]"
    Weekday(Ordinal, Dow), // "first monday"
}

impl MonthOccurrence {
    pub fn matches(&self, t: NaiveDate) -> bool {
        match self {
            Self::Day(ordinal) => {
                if ordinal.matches(t) {
                    true
                } else if let Ordinal::Nth(Nth(n)) = ordinal && *n > t.num_days_in_month() && t.is_month_end() {
                    // schedule is something like "every month on 30th", but we're now in february,
                    // which doesn't have 30 days. We should match on the last day of Feb
                    true
                } else {
                    false
                }
            }
            Self::Weekday(ordinal, dow) => {
                if !dow.matches(t) {
                    return false;
                }
                match ordinal {
                    Ordinal::First => {
                        // We know we're on the right day of week, so if this is to be the first
                        // occurrence, we have to be in the first week of the month
                        t.day() < 8
                    }
                    Ordinal::Nth(Nth(n)) => {
                        // no months have more than 5 of any weekday
                        if *n > 5 {
                            return false;
                        }
                        (t.day() - 1) / 7 + 1 == (*n).into()
                    }
                    Ordinal::Last => {
                        // Similar to First, we need to be within a week of the end of the month
                        t.num_days_in_month() as u32 - t.day() < 8
                    }
                }
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Period {
    Day,
    Week { on: Vec<Dow> },
    Weekday(Dow),                                      // "every monday"
    NamedMonth { month: Month, day: Option<Ordinal> }, // "every january [1st]"
    Month { on: Vec<MonthOccurrence> },
    Quarter,
    Year { on: Vec<(Month, Ordinal)> },
}

#[derive(Debug, Clone, PartialEq)]
pub struct Periodic {
    pub nth: Option<Nth>,
    pub period: Period,
    pub start: Option<NaiveDate>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Schedule {
    Periodic(Periodic),
    Dates(Vec<NaiveDate>),
}

impl Schedule {
    pub fn matches(&self, t: NaiveDate) -> bool {
        let periodic = match self {
            Self::Dates(dates) => return dates.contains(&t),
            Self::Periodic(periodic) => periodic,
        };

        if periodic.start.is_some_and(|s| t < s) {
            return false;
        }

        let origin = periodic.start.unwrap_or(t);

        match &periodic.period {
            Period::Day => match periodic.nth {
                None => true,
                Some(Nth(n)) => {
                    (t - origin).num_days() % n as i64 == 0
                }
            },
            Period::Week { on } => {
                let dow_ok = if on.is_empty() {
                    Dow::Monday.matches(t)
                } else {
                    on.iter().any(|dow| dow.matches(t))
                };
                if !dow_ok {
                    return false;
                }
                match periodic.nth {
                    None => true,
                    Some(Nth(n)) => {
                        (t - origin).num_days() / 7 % n as i64 == 0
                    }
                }
            }
            Period::Weekday(dow) => {
                if !dow.matches(t) {
                    return false;
                }
                match periodic.nth {
                    None => true,
                    Some(Nth(n)) => {
                        (t - origin).num_days() / 7 % n as i64 == 0
                    }
                }
            }
            Period::Month { on } => {
                let day_ok = if on.is_empty() {
                    t.is_month_end()
                } else {
                    on.iter().any(|d| d.matches(t))
                };
                if !day_ok {
                    return false;
                }
                match periodic.nth {
                    None => true,
                    Some(Nth(n)) => {
                        let months = (t.year() - origin.year()) * 12
                            + t.month() as i32 - origin.month() as i32;
                        months % n as i32 == 0
                    }
                }
            }
            Period::NamedMonth { month, day } => {
                if !month.matches(t) {
                    return false;
                }
                let day_ok = match day {
                    Some(ordinal) => ordinal.matches(t),
                    None => t.is_month_end(),
                };
                if !day_ok {
                    return false;
                }
                match periodic.nth {
                    None => true,
                    Some(Nth(n)) => {
                        (t.year() - origin.year()) % n as i32 == 0
                    }
                }
            }
            Period::Quarter => {
                if !t.is_quarter_end() {
                    return false;
                }
                match periodic.nth {
                    None => true,
                    Some(Nth(n)) => {
                        let quarters = (t.year() - origin.year()) * 4
                            + t.quarter() as i32 - origin.quarter() as i32;
                        quarters % n as i32 == 0
                    }
                }
            }
            Period::Year { on } => {
                let day_ok = if on.is_empty() {
                    t.is_year_end()
                } else {
                    on.iter().any(|(month, ordinal)| month.matches(t) && ordinal.matches(t))
                };
                if !day_ok {
                    return false;
                }
                match periodic.nth {
                    None => true,
                    Some(Nth(n)) => {
                        (t.year() - origin.year()) % n as i32 == 0
                    }
                }
            }
        }
    }
}

trait PeriodEnd: Datelike {
    fn is_month_end(&self) -> bool {
        self.day() == self.num_days_in_month() as u32
    }
    fn is_quarter_end(&self) -> bool {
        match self.month() {
            3 | 12 => self.day() == 31,
            6 | 9 => self.day() == 30,
            _ => false,
        }
    }
    fn is_year_end(&self) -> bool {
        self.month() == 12 && self.day() == 31
    }
}

impl PeriodEnd for NaiveDate {}

#[cfg(test)]
mod tests {
    use super::*;

    fn date(y: i32, m: u32, d: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, d).unwrap()
    }

    mod dates {
        use super::*;

        #[test]
        fn matches_single_date() {
            let schedule = Schedule::Dates(vec![date(2025, 1, 1)]);
            assert!(schedule.matches(date(2025, 1, 1)));
            assert!(!schedule.matches(date(2025, 1, 2)));
            assert!(!schedule.matches(date(2025, 1, 3)));
        }

        #[test]
        fn matches_multiple_date() {
            let schedule = Schedule::Dates(vec![date(2025, 1, 1), date(2025, 1, 3)]);
            assert!(schedule.matches(date(2025, 1, 1)));
            assert!(!schedule.matches(date(2025, 1, 2)));
            assert!(schedule.matches(date(2025, 1, 3)));
        }

        #[test]
        fn doesnt_match_empty_dates() {
            let schedule = Schedule::Dates(vec![]);
            assert!(!schedule.matches(date(2025, 1, 1)));
            assert!(!schedule.matches(date(2025, 1, 2)));
            assert!(!schedule.matches(date(2025, 1, 3)));
        }
    }

    mod periodic {
        use super::*;

        mod simple {
            use super::*;

            fn schedule(period: Period) -> Schedule {
                Schedule::Periodic(Periodic {
                    nth: None,
                    period,
                    start: None,
                })
            }

            mod day {
                use super::*;

                #[test]
                fn matches_every_day() {
                    let sched = schedule(Period::Day);
                    assert!(sched.matches(date(2025, 1, 1)));
                    assert!(sched.matches(date(2025, 1, 2)));
                    assert!(sched.matches(date(2025, 1, 3)));
                }
            }

            mod week {
                use super::*;

                #[test]
                fn matches_without_on() {
                    let sched = schedule(Period::Week { on: vec![] });
                    assert!(sched.matches(date(2024, 12, 30))); // monday
                    assert!(!sched.matches(date(2025, 1, 1))); // wednesday
                    assert!(sched.matches(date(2025, 1, 6))); // monday
                }

                #[test]
                fn matches_with_single_on() {
                    let sched = schedule(Period::Week {
                        on: vec![Dow::Wednesday],
                    });
                    assert!(!sched.matches(date(2024, 12, 30))); // monday
                    assert!(sched.matches(date(2025, 1, 1))); // wednesday
                    assert!(!sched.matches(date(2025, 1, 6))); // monday
                    assert!(sched.matches(date(2025, 1, 8))); // wednesday
                }

                #[test]
                fn matches_with_multi_on() {
                    let sched = schedule(Period::Week {
                        on: vec![Dow::Monday, Dow::Wednesday],
                    });
                    assert!(sched.matches(date(2024, 12, 30))); // monday
                    assert!(!sched.matches(date(2024, 12, 31))); // monday
                    assert!(sched.matches(date(2025, 1, 1))); // wednesday
                    assert!(!sched.matches(date(2025, 1, 2))); // thursday
                }

                #[test]
                fn matches_with_weekday() {
                    let sched = schedule(Period::Week {
                        on: vec![Dow::Weekday],
                    });
                    assert!(sched.matches(date(2024, 12, 31))); // monday
                    assert!(sched.matches(date(2025, 1, 1))); // wednesday
                    assert!(sched.matches(date(2025, 1, 2))); // thursday
                    assert!(!sched.matches(date(2025, 1, 4))); // saturday
                    assert!(!sched.matches(date(2025, 1, 5))); // sunday
                }

                #[test]
                fn matches_with_weekend() {
                    let sched = schedule(Period::Week {
                        on: vec![Dow::Weekend],
                    });
                    assert!(!sched.matches(date(2024, 12, 31))); // monday
                    assert!(!sched.matches(date(2025, 1, 1))); // wednesday
                    assert!(!sched.matches(date(2025, 1, 2))); // thursday
                    assert!(sched.matches(date(2025, 1, 4))); // saturday
                    assert!(sched.matches(date(2025, 1, 5))); // sunday
                }
            }

            mod weekday {
                use super::*;

                #[test]
                fn matches() {
                    let sched = schedule(Period::Weekday(Dow::Wednesday));
                    assert!(!sched.matches(date(2024, 12, 30))); // monday
                    assert!(sched.matches(date(2025, 1, 1))); // wednesday
                    assert!(!sched.matches(date(2025, 1, 6))); // monday
                    assert!(sched.matches(date(2025, 1, 8))); // wednesday
                }
            }

            mod named_month {
                use super::*;

                #[test]
                fn matches_without_ordinal() {
                    let sched = schedule(Period::NamedMonth {
                        month: Month::February,
                        day: None,
                    });

                    assert!(!sched.matches(date(2024, 2, 28)));
                    assert!(sched.matches(date(2024, 2, 29)));
                    assert!(sched.matches(date(2025, 2, 28)));
                    assert!(!sched.matches(date(2025, 3, 31)));
                }

                #[test]
                fn matches_with_first_ordinal() {
                    let sched = schedule(Period::NamedMonth {
                        month: Month::February,
                        day: Some(Ordinal::First),
                    });
                    assert!(sched.matches(date(2024, 2, 1)));
                    assert!(sched.matches(date(2025, 2, 1)));
                    assert!(!sched.matches(date(2025, 3, 1)));
                }

                #[test]
                fn matches_with_nth_ordinal() {
                    let sched = schedule(Period::NamedMonth {
                        month: Month::February,
                        day: Some(Ordinal::Nth(Nth(2))),
                    });
                    assert!(sched.matches(date(2024, 2, 2)));
                    assert!(sched.matches(date(2025, 2, 2)));
                    assert!(!sched.matches(date(2025, 3, 2)));
                }

                #[test]
                fn matches_with_last_ordinal() {
                    let sched = schedule(Period::NamedMonth {
                        month: Month::February,
                        day: Some(Ordinal::Last),
                    });
                    assert!(!sched.matches(date(2024, 2, 28)));
                    assert!(sched.matches(date(2024, 2, 29)));
                    assert!(sched.matches(date(2025, 2, 28)));
                    assert!(!sched.matches(date(2025, 3, 31)));
                }
            }

            mod month {
                use super::*;

                #[test]
                fn matches_without_on() {
                    let sched = schedule(Period::Month { on: vec![] });
                    assert!(sched.matches(date(2024, 1, 31)));
                    assert!(sched.matches(date(2024, 2, 29)));
                    assert!(sched.matches(date(2024, 3, 31)));
                    assert!(!sched.matches(date(2024, 3, 1)));
                }

                #[test]
                fn matches_with_day_on() {
                    let sched = schedule(Period::Month {
                        on: vec![MonthOccurrence::Day(Ordinal::Nth(Nth(30)))],
                    });
                    assert!(!sched.matches(date(2024, 1, 29)));
                    assert!(sched.matches(date(2024, 1, 30)));
                    assert!(!sched.matches(date(2024, 1, 31)));
                    assert!(sched.matches(date(2024, 2, 29)));
                    assert!(sched.matches(date(2024, 3, 30)));
                }

                #[test]
                fn matches_with_first_weekday_on() {
                    let sched = schedule(Period::Month {
                        on: vec![MonthOccurrence::Weekday(Ordinal::First, Dow::Monday)],
                    });
                    assert!(sched.matches(date(2024, 1, 1))); // first monday
                    assert!(!sched.matches(date(2024, 1, 2))); // first tuesday
                    assert!(!sched.matches(date(2024, 1, 8))); // second monday
                }

                #[test]
                fn matches_with_nth_weekday_on() {
                    let sched = schedule(Period::Month {
                        on: vec![MonthOccurrence::Weekday(Ordinal::Nth(Nth(2)), Dow::Monday)],
                    });
                    assert!(!sched.matches(date(2024, 1, 1))); // first monday
                    assert!(!sched.matches(date(2024, 1, 2))); // first tuesday
                    assert!(sched.matches(date(2024, 1, 8))); // second monday
                }

                #[test]
                fn matches_with_last_weekday_on() {
                    let sched = schedule(Period::Month {
                        on: vec![MonthOccurrence::Weekday(Ordinal::Last, Dow::Monday)],
                    });
                    assert!(!sched.matches(date(2024, 1, 22))); // second-to-last monday
                    assert!(sched.matches(date(2024, 1, 29))); // last monday
                }
            }

            mod quarter {
                use super::*;

                #[test]
                fn matches() {
                    let sched = schedule(Period::Quarter);
                    assert!(!sched.matches(date(2024, 1, 31)));
                    assert!(!sched.matches(date(2024, 2, 29)));
                    assert!(sched.matches(date(2024, 3, 31)));
                    assert!(!sched.matches(date(2024, 4, 30)));
                    assert!(!sched.matches(date(2024, 5, 31)));
                    assert!(sched.matches(date(2024, 6, 30)));
                    assert!(!sched.matches(date(2024, 7, 31)));
                    assert!(!sched.matches(date(2024, 8, 31)));
                    assert!(sched.matches(date(2024, 9, 30)));
                    assert!(!sched.matches(date(2024, 10, 31)));
                    assert!(!sched.matches(date(2024, 11, 30)));
                    assert!(sched.matches(date(2024, 12, 31)));
                }
            }

            mod year {
                use super::*;

                #[test]
                fn matches_without_on() {
                    let sched = schedule(Period::Year { on: Vec::new() });
                    assert!(!sched.matches(date(2024, 1, 31)));
                    assert!(!sched.matches(date(2024, 2, 29)));
                    assert!(!sched.matches(date(2024, 3, 31)));
                    assert!(!sched.matches(date(2024, 4, 30)));
                    assert!(!sched.matches(date(2024, 5, 31)));
                    assert!(!sched.matches(date(2024, 6, 30)));
                    assert!(!sched.matches(date(2024, 7, 31)));
                    assert!(!sched.matches(date(2024, 8, 31)));
                    assert!(!sched.matches(date(2024, 9, 30)));
                    assert!(!sched.matches(date(2024, 10, 31)));
                    assert!(!sched.matches(date(2024, 11, 30)));
                    assert!(sched.matches(date(2024, 12, 31)));
                }

                #[test]
                fn matches_with_single_on() {
                    let sched = schedule(Period::Year { on: vec![(Month::May, Ordinal::Last)] });
                    assert!(!sched.matches(date(2024, 1, 31)));
                    assert!(!sched.matches(date(2024, 2, 29)));
                    assert!(!sched.matches(date(2024, 3, 31)));
                    assert!(!sched.matches(date(2024, 4, 30)));
                    assert!(sched.matches(date(2024, 5, 31)));
                    assert!(!sched.matches(date(2024, 6, 30)));
                    assert!(!sched.matches(date(2024, 7, 31)));
                    assert!(!sched.matches(date(2024, 8, 31)));
                    assert!(!sched.matches(date(2024, 9, 30)));
                    assert!(!sched.matches(date(2024, 10, 31)));
                    assert!(!sched.matches(date(2024, 11, 30)));
                    assert!(!sched.matches(date(2024, 12, 31)));
                }

                #[test]
                fn matches_with_multiple_on() {
                    let sched = schedule(Period::Year { on: vec![(Month::February, Ordinal::Last), (Month::May, Ordinal::Last)] });
                    assert!(!sched.matches(date(2024, 1, 31)));
                    assert!(sched.matches(date(2024, 2, 29)));
                    assert!(!sched.matches(date(2024, 3, 31)));
                    assert!(!sched.matches(date(2024, 4, 30)));
                    assert!(sched.matches(date(2024, 5, 31)));
                    assert!(!sched.matches(date(2024, 6, 30)));
                    assert!(!sched.matches(date(2024, 7, 31)));
                    assert!(!sched.matches(date(2024, 8, 31)));
                    assert!(!sched.matches(date(2024, 9, 30)));
                    assert!(!sched.matches(date(2024, 10, 31)));
                    assert!(!sched.matches(date(2024, 11, 30)));
                    assert!(!sched.matches(date(2024, 12, 31)));
                }

            }
        }

        mod nth {
            use super::*;

            fn schedule(nth: u8, period: Period, start: NaiveDate) -> Schedule {
                Schedule::Periodic(Periodic {
                    nth: Some(Nth(nth)),
                    period,
                    start: Some(start),
                })
            }

            mod day {
                use super::*;

                #[test]
                fn every_2_days() {
                    let sched = schedule(2, Period::Day, date(2025, 1, 1));
                    assert!(sched.matches(date(2025, 1, 1)));  // day 0
                    assert!(!sched.matches(date(2025, 1, 2))); // day 1
                    assert!(sched.matches(date(2025, 1, 3)));  // day 2
                    assert!(!sched.matches(date(2025, 1, 4))); // day 3
                    assert!(sched.matches(date(2025, 1, 5)));  // day 4
                }

                #[test]
                fn every_3_days() {
                    let sched = schedule(3, Period::Day, date(2025, 1, 1));
                    assert!(sched.matches(date(2025, 1, 1)));  // day 0
                    assert!(!sched.matches(date(2025, 1, 2))); // day 1
                    assert!(!sched.matches(date(2025, 1, 3))); // day 2
                    assert!(sched.matches(date(2025, 1, 4)));  // day 3
                    assert!(!sched.matches(date(2025, 1, 5))); // day 4
                    assert!(!sched.matches(date(2025, 1, 6))); // day 5
                    assert!(sched.matches(date(2025, 1, 7)));  // day 6
                }

                #[test]
                fn start_is_respected() {
                    let sched = schedule(2, Period::Day, date(2025, 1, 3));
                    assert!(!sched.matches(date(2025, 1, 1))); // before start
                    assert!(!sched.matches(date(2025, 1, 2))); // before start
                    assert!(sched.matches(date(2025, 1, 3)));  // day 0
                    assert!(!sched.matches(date(2025, 1, 4))); // day 1
                    assert!(sched.matches(date(2025, 1, 5)));  // day 2
                }
            }

            mod weekday {
                use super::*;

                #[test]
                fn every_2_mondays() {
                    // start = Mon 2025-01-06
                    let sched = schedule(2, Period::Weekday(Dow::Monday), date(2025, 1, 6));
                    assert!(sched.matches(date(2025, 1, 6)));   // occurrence 0
                    assert!(!sched.matches(date(2025, 1, 7)));  // Tuesday — wrong dow
                    assert!(!sched.matches(date(2025, 1, 13))); // occurrence 1, skip
                    assert!(sched.matches(date(2025, 1, 20)));  // occurrence 2
                    assert!(!sched.matches(date(2025, 1, 27))); // occurrence 3, skip
                    assert!(sched.matches(date(2025, 2, 3)));   // occurrence 4
                }

                #[test]
                fn every_3_wednesdays() {
                    // start = Wed 2025-01-01
                    let sched = schedule(3, Period::Weekday(Dow::Wednesday), date(2025, 1, 1));
                    assert!(sched.matches(date(2025, 1, 1)));   // occurrence 0
                    assert!(!sched.matches(date(2025, 1, 8)));  // occurrence 1, skip
                    assert!(!sched.matches(date(2025, 1, 15))); // occurrence 2, skip
                    assert!(sched.matches(date(2025, 1, 22)));  // occurrence 3
                }

                #[test]
                fn start_not_on_target_dow() {
                    // start = Tue 2025-01-07, target = Monday, every 2
                    let sched = schedule(2, Period::Weekday(Dow::Monday), date(2025, 1, 7));
                    assert!(!sched.matches(date(2025, 1, 6)));  // before start
                    assert!(sched.matches(date(2025, 1, 13)));  // first Monday at/after start — occurrence 0
                    assert!(!sched.matches(date(2025, 1, 20))); // occurrence 1, skip
                    assert!(sched.matches(date(2025, 1, 27)));  // occurrence 2
                }
            }

            mod month {
                use super::*;

                #[test]
                fn every_2_months_no_on() {
                    // no on → fires on month end
                    let sched = schedule(2, Period::Month { on: vec![] }, date(2025, 1, 1));
                    assert!(sched.matches(date(2025, 1, 31)));  // month 0 end
                    assert!(!sched.matches(date(2025, 2, 28))); // month 1, skip
                    assert!(sched.matches(date(2025, 3, 31)));  // month 2
                    assert!(!sched.matches(date(2025, 4, 30))); // month 3, skip
                    assert!(sched.matches(date(2025, 5, 31)));  // month 4
                }

                #[test]
                fn every_3_months_on_15th() {
                    let sched = schedule(3, Period::Month { on: vec![MonthOccurrence::Day(Ordinal::Nth(Nth(15)))] }, date(2025, 1, 1));
                    assert!(sched.matches(date(2025, 1, 15)));  // month 0
                    assert!(!sched.matches(date(2025, 2, 15))); // month 1, skip
                    assert!(!sched.matches(date(2025, 3, 15))); // month 2, skip
                    assert!(sched.matches(date(2025, 4, 15)));  // month 3
                    assert!(!sched.matches(date(2025, 1, 16))); // month 0, wrong day
                }

                #[test]
                fn every_2_months_on_first_monday() {
                    let sched = schedule(2, Period::Month { on: vec![MonthOccurrence::Weekday(Ordinal::First, Dow::Monday)] }, date(2025, 1, 1));
                    assert!(sched.matches(date(2025, 1, 6)));   // month 0 — first Monday of Jan
                    assert!(!sched.matches(date(2025, 2, 3)));  // month 1, skip
                    assert!(sched.matches(date(2025, 3, 3)));   // month 2 — first Monday of Mar
                    assert!(!sched.matches(date(2025, 4, 7)));  // month 3, skip
                }
            }

            mod quarter {
                use super::*;

                #[test]
                fn every_2_quarters() {
                    // start = 2025-01-01 (Q1)
                    let sched = schedule(2, Period::Quarter, date(2025, 1, 1));
                    assert!(sched.matches(date(2025, 3, 31)));   // quarter 0 (Q1)
                    assert!(!sched.matches(date(2025, 6, 30)));  // quarter 1, skip
                    assert!(sched.matches(date(2025, 9, 30)));   // quarter 2 (Q3)
                    assert!(!sched.matches(date(2025, 12, 31))); // quarter 3, skip
                    assert!(sched.matches(date(2026, 3, 31)));   // quarter 4
                }

                #[test]
                fn every_3_quarters() {
                    let sched = schedule(3, Period::Quarter, date(2025, 1, 1));
                    assert!(sched.matches(date(2025, 3, 31)));   // quarter 0
                    assert!(!sched.matches(date(2025, 6, 30)));  // quarter 1, skip
                    assert!(!sched.matches(date(2025, 9, 30)));  // quarter 2, skip
                    assert!(sched.matches(date(2025, 12, 31)));  // quarter 3
                    assert!(!sched.matches(date(2026, 3, 31)));  // quarter 4, skip
                    assert!(sched.matches(date(2026, 9, 30)));   // quarter 6
                }

                #[test]
                fn start_in_q2() {
                    // start = 2025-04-01 (Q2), every 2
                    let sched = schedule(2, Period::Quarter, date(2025, 4, 1));
                    assert!(!sched.matches(date(2025, 3, 31)));  // before start
                    assert!(sched.matches(date(2025, 6, 30)));   // quarter 0 (Q2)
                    assert!(!sched.matches(date(2025, 9, 30)));  // quarter 1, skip
                    assert!(sched.matches(date(2025, 12, 31)));  // quarter 2 (Q4)
                    assert!(!sched.matches(date(2026, 3, 31)));  // quarter 3, skip
                }
            }

            mod year {
                use super::*;

                #[test]
                fn every_2_years_no_on() {
                    // no on → year end (Dec 31)
                    let sched = schedule(2, Period::Year { on: Vec::new() }, date(2024, 1, 1));
                    assert!(sched.matches(date(2024, 12, 31)));  // year 0
                    assert!(!sched.matches(date(2025, 12, 31))); // year 1, skip
                    assert!(sched.matches(date(2026, 12, 31)));  // year 2
                    assert!(!sched.matches(date(2027, 12, 31))); // year 3, skip
                }

                #[test]
                fn every_3_years_with_on() {
                    let sched = schedule(3, Period::Year { on: vec![(Month::May, Ordinal::Last)] }, date(2024, 1, 1));
                    assert!(sched.matches(date(2024, 5, 31)));   // year 0
                    assert!(!sched.matches(date(2025, 5, 31)));  // year 1, skip
                    assert!(!sched.matches(date(2026, 5, 31)));  // year 2, skip
                    assert!(sched.matches(date(2027, 5, 31)));   // year 3
                    assert!(!sched.matches(date(2024, 6, 30)));  // year 0, wrong month
                }
            }

            mod named_month {
                use super::*;

                #[test]
                fn every_2_years_in_february() {
                    // start = 2024-02-29; no day → month end
                    let sched = schedule(2, Period::NamedMonth { month: Month::February, day: None }, date(2024, 2, 29));
                    assert!(sched.matches(date(2024, 2, 29)));  // year 0 — leap year end
                    assert!(!sched.matches(date(2025, 2, 28))); // year 1, skip
                    assert!(sched.matches(date(2026, 2, 28)));  // year 2
                    assert!(!sched.matches(date(2027, 2, 28))); // year 3, skip
                    assert!(sched.matches(date(2028, 2, 29)));  // year 4
                }

                #[test]
                fn every_3_years_in_march_on_15th() {
                    let sched = schedule(3, Period::NamedMonth { month: Month::March, day: Some(Ordinal::Nth(Nth(15))) }, date(2024, 3, 15));
                    assert!(sched.matches(date(2024, 3, 15)));  // year 0
                    assert!(!sched.matches(date(2025, 3, 15))); // year 1, skip
                    assert!(!sched.matches(date(2026, 3, 15))); // year 2, skip
                    assert!(sched.matches(date(2027, 3, 15)));  // year 3
                    assert!(!sched.matches(date(2024, 4, 15))); // right day, wrong month
                }
            }

            mod week {
                use super::*;

                // start = Mon 2025-01-06; every 2 weeks (no explicit on → defaults to Monday)
                #[test]
                fn every_2_weeks_default_monday() {
                    let sched = schedule(2, Period::Week { on: vec![] }, date(2025, 1, 6));
                    assert!(sched.matches(date(2025, 1, 6)));   // week 0 — Mon
                    assert!(!sched.matches(date(2025, 1, 13))); // week 1 — Mon, skip
                    assert!(sched.matches(date(2025, 1, 20)));  // week 2 — Mon
                    assert!(!sched.matches(date(2025, 1, 27))); // week 3 — Mon, skip
                    assert!(sched.matches(date(2025, 2, 3)));   // week 4 — Mon
                }

                #[test]
                fn every_2_weeks_on_wednesday() {
                    // start = Mon 2025-01-06; on = Wednesday
                    let sched = schedule(2, Period::Week { on: vec![Dow::Wednesday] }, date(2025, 1, 6));
                    assert!(!sched.matches(date(2025, 1, 6)));  // week 0 — Mon, wrong dow
                    assert!(sched.matches(date(2025, 1, 8)));   // week 0 — Wed
                    assert!(!sched.matches(date(2025, 1, 15))); // week 1 — Wed, skip
                    assert!(sched.matches(date(2025, 1, 22)));  // week 2 — Wed
                    assert!(!sched.matches(date(2025, 1, 29))); // week 3 — Wed, skip
                }

                #[test]
                fn every_3_weeks() {
                    let sched = schedule(3, Period::Week { on: vec![] }, date(2025, 1, 6));
                    assert!(sched.matches(date(2025, 1, 6)));   // week 0
                    assert!(!sched.matches(date(2025, 1, 13))); // week 1
                    assert!(!sched.matches(date(2025, 1, 20))); // week 2
                    assert!(sched.matches(date(2025, 1, 27)));  // week 3
                }
            }
        }
    }
}
