use chrono::NaiveDate;

#[derive(Debug, Clone, PartialEq)]
pub enum Schedule {
    Every(Every),
    Dates(Vec<NaiveDate>),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Every {
    pub nth: Option<Ordinal>,
    pub period: Period,
    pub start: Option<NaiveDate>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Period {
    Day,
    Week { on: Vec<Dow> },
    Weekday(Dow),                                      // "every monday"
    NamedMonth { month: Month, day: Option<Ordinal> }, // "every january [1st]"
    Month { on: Vec<MonthOccurrence> },
    Quarter,
    Year { on: Option<(Month, Ordinal)> },
}

#[derive(Debug, Clone, PartialEq)]
pub enum MonthOccurrence {
    Day(Ordinal),          // "1st [day]"
    Weekday(Ordinal, Dow), // "first monday"
}

#[derive(Debug, Clone, PartialEq)]
pub enum Ordinal {
    Nth(u8),
    Last,
}

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
    WeekendDay,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Month {
    January,
    February,
    March,
    April,
    May,
    June,
    July,
    August,
    September,
    October,
    November,
    December,
}
