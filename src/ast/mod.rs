pub mod schedule;

use chrono::NaiveDate;
use rust_decimal::Decimal;
pub use schedule::Schedule;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Span {
    pub start: usize,
    pub end: usize,
}

impl Span {
    pub fn new(start: usize, end: usize) -> Self {
        Span { start, end }
    }

    pub fn merge(self, other: Self) -> Self {
        Span {
            start: self.start,
            end: other.end,
        }
    }

    pub fn into_range(self) -> std::ops::Range<usize> {
        self.start..self.end
    }
}

impl std::fmt::Display for Span {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}..{}", self.start, self.end)
    }
}

impl From<std::ops::Range<usize>> for Span {
    fn from(r: std::ops::Range<usize>) -> Self {
        Span {
            start: r.start,
            end: r.end,
        }
    }
}

pub type Spanned<T> = (T, Span);
pub type SpannedExpr = Spanned<Box<Expr>>;

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Path(pub Vec<String>);

impl Path {
    pub fn join(&self) -> String {
        self.0.join(":")
    }
}

impl std::fmt::Display for Path {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.join())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum AggKind {
    Ytd,
    Qtd,
    Mtd,
}

impl AggKind {
    /// The period the total accumulates over.
    pub fn unit(self) -> TimeUnit {
        match self {
            AggKind::Ytd => TimeUnit::Year,
            AggKind::Qtd => TimeUnit::Quarter,
            AggKind::Mtd => TimeUnit::Month,
        }
    }

    /// The first day of the period containing `t`.
    pub fn period_start(self, t: NaiveDate) -> NaiveDate {
        self.unit().period_start(t)
    }
}

/// A unit of calendar time. Each is also a calendar period: weeks run Monday to
/// Sunday, and quarters start in January, April, July and October.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum TimeUnit {
    Day,
    Week,
    Month,
    Quarter,
    Year,
}

impl TimeUnit {
    pub fn from_name(name: &str) -> Option<Self> {
        Some(match name {
            "day" => TimeUnit::Day,
            "week" => TimeUnit::Week,
            "month" => TimeUnit::Month,
            "quarter" => TimeUnit::Quarter,
            "year" => TimeUnit::Year,
            _ => return None,
        })
    }

    pub fn name(self) -> &'static str {
        match self {
            TimeUnit::Day => "day",
            TimeUnit::Week => "week",
            TimeUnit::Month => "month",
            TimeUnit::Quarter => "quarter",
            TimeUnit::Year => "year",
        }
    }

    /// The first day of the period containing `t`.
    pub fn period_start(self, t: NaiveDate) -> NaiveDate {
        use chrono::Datelike;
        let first_of = |month| NaiveDate::from_ymd_opt(t.year(), month, 1).expect("valid date");
        match self {
            TimeUnit::Day => t,
            TimeUnit::Week => {
                let days = t.weekday().num_days_from_monday();
                t.checked_sub_days(chrono::Days::new(days.into()))
                    .unwrap_or(NaiveDate::MIN)
            }
            TimeUnit::Month => first_of(t.month()),
            TimeUnit::Quarter => first_of((t.month() - 1) / 3 * 3 + 1),
            TimeUnit::Year => first_of(1),
        }
    }

    /// Numbers periods consecutively, so subtracting the numbers of two dates
    /// gives how many periods apart they are.
    pub fn period_number(self, t: NaiveDate) -> i64 {
        use chrono::Datelike;
        let months = i64::from(t.year()) * 12 + i64::from(t.month0());
        match self {
            TimeUnit::Day => t.num_days_from_ce().into(),
            // Day 1 of the common era was a Monday.
            TimeUnit::Week => (i64::from(t.num_days_from_ce()) - 1).div_euclid(7),
            TimeUnit::Month => months,
            TimeUnit::Quarter => months.div_euclid(3),
            TimeUnit::Year => t.year().into(),
        }
    }

    /// Converting an amount per `self` into an amount per `other` multiplies it
    /// by the first number and divides it by the second. `None` when neither
    /// unit is a whole number of the other, like weeks and years.
    pub fn conversion_to(self, other: TimeUnit) -> Option<(u32, u32)> {
        /// Length in days for days and weeks, in months for the rest.
        fn length(unit: TimeUnit) -> (bool, u32) {
            match unit {
                TimeUnit::Day => (false, 1),
                TimeUnit::Week => (false, 7),
                TimeUnit::Month => (true, 1),
                TimeUnit::Quarter => (true, 3),
                TimeUnit::Year => (true, 12),
            }
        }
        let ((months, from), (other_months, to)) = (length(self), length(other));
        if months != other_months {
            return None;
        }
        // Within days and weeks, or months, quarters and years, the shorter
        // length divides the longer.
        let shorter = from.min(to);
        Some((to / shorter, from / shorter))
    }
}

impl std::fmt::Display for TimeUnit {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.name())
    }
}

impl std::fmt::Display for AggKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            AggKind::Ytd => "ytd",
            AggKind::Qtd => "qtd",
            AggKind::Mtd => "mtd",
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
    Lt,
    LtEq,
    Gt,
    GtEq,
    Eq,
    NotEq,
    And,
    Or,
}

impl std::fmt::Display for BinOp {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            BinOp::Add => "+",
            BinOp::Sub => "-",
            BinOp::Mul => "*",
            BinOp::Div => "/",
            BinOp::Lt => "<",
            BinOp::LtEq => "<=",
            BinOp::Gt => ">",
            BinOp::GtEq => ">=",
            BinOp::Eq => "==",
            BinOp::NotEq => "!=",
            BinOp::And => "and",
            BinOp::Or => "or",
        })
    }
}

#[derive(Clone, Debug)]
pub enum Expr {
    Num(Decimal),
    Bool(bool),
    Ref(Path),
    Neg(SpannedExpr),
    Not(SpannedExpr),
    Bin(SpannedExpr, BinOp, SpannedExpr),
    If {
        cond: SpannedExpr,
        then: SpannedExpr,
        else_: SpannedExpr,
    },
    Call(String, Vec<SpannedExpr>),
    /// `x per year`: an amount per period.
    Per(SpannedExpr, TimeUnit),
    /// `.ytd`/`.qtd`/`.mtd` aggregation. The optional first field is the entry
    /// qualifier (e.g. `paycheck.k401_contrib.ytd` has `Some("paycheck")`).
    /// Unqualified form (`k401_contrib.ytd`) is only valid inside the defining entry.
    ParamAgg(Option<String>, String, AggKind),
}

#[derive(Clone, Debug)]
pub enum ScheduleRef {
    Literal(Schedule),
    Named(String, Span),
}

#[derive(Clone, Debug)]
pub struct Interval {
    pub from: NaiveDate,
    pub to: Option<NaiveDate>,
    pub value: SpannedExpr,
    pub span: Span,
}

#[derive(Clone, Debug)]
pub enum PostingAmount {
    Expr(SpannedExpr),
    All,
}

#[derive(Clone, Debug)]
pub struct Posting {
    pub account: Path,
    pub amount: Option<PostingAmount>,
    pub leg_name: Option<String>,
    /// The whole posting line.
    pub span: Span,
    pub account_span: Span,
    pub leg_span: Option<Span>,
}

#[derive(Clone, Debug)]
pub enum ParamBody {
    Const(SpannedExpr),
    Schedule(Vec<Interval>),
}

#[derive(Clone, Debug)]
pub enum Stmt {
    Let { name: String, value: SpannedExpr },
    Return(SpannedExpr),
}

#[derive(Clone, Debug)]
pub enum Decl {
    Account {
        name: Path,
        opening: Option<(SpannedExpr, NaiveDate)>,
    },
    Schedule {
        name: String,
        schedule: Schedule,
    },
    Param {
        name: String,
        body: ParamBody,
    },
    Entry {
        label: String,
        alias: Option<String>,
        schedule: ScheduleRef,
        postings: Vec<Posting>,
    },
    Assert {
        schedule: Option<ScheduleRef>,
        asserted: SpannedExpr,
    },
    Fn {
        name: String,
        params: Vec<Spanned<String>>,
        body: Vec<Stmt>,
    },
}

#[derive(Clone, Debug)]
pub struct Program {
    pub decls: Vec<Spanned<Decl>>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn date(y: i32, m: u32, d: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, d).unwrap()
    }

    #[test]
    fn periods_start_on_calendar_boundaries() {
        let t = date(2026, 8, 13); // a Thursday
        let starts = [
            (TimeUnit::Day, date(2026, 8, 13)),
            (TimeUnit::Week, date(2026, 8, 10)),
            (TimeUnit::Month, date(2026, 8, 1)),
            (TimeUnit::Quarter, date(2026, 7, 1)),
            (TimeUnit::Year, date(2026, 1, 1)),
        ];
        for (unit, start) in starts {
            assert_eq!(unit.period_start(t), start, "{unit}");
        }
    }

    #[test]
    fn period_numbers_count_periods_between_dates() {
        let (sun, mon) = (date(2026, 8, 16), date(2026, 8, 17));
        let week = |t| TimeUnit::Week.period_number(t);
        assert_eq!(week(mon) - week(sun), 1);
        assert_eq!(week(sun) - week(date(2026, 8, 10)), 0);
        assert_eq!(week(date(1, 1, 7)) - week(date(1, 1, 1)), 0);
        let quarter = |t| TimeUnit::Quarter.period_number(t);
        assert_eq!(quarter(date(2027, 1, 1)) - quarter(date(2026, 12, 31)), 1);
        assert_eq!(quarter(date(-1, 12, 31)) - quarter(date(0, 1, 1)), -1);
    }

    #[test]
    fn converts_between_units_that_divide_evenly() {
        use TimeUnit::*;
        assert_eq!(Month.conversion_to(Year), Some((12, 1)));
        assert_eq!(Year.conversion_to(Month), Some((1, 12)));
        assert_eq!(Quarter.conversion_to(Year), Some((4, 1)));
        assert_eq!(Day.conversion_to(Week), Some((7, 1)));
        assert_eq!(Year.conversion_to(Year), Some((1, 1)));
        assert_eq!(Week.conversion_to(Year), None);
        assert_eq!(Day.conversion_to(Month), None);
    }

    #[test]
    fn fmt_path() {
        assert_eq!("Single", Path(vec!["Single".to_string()]).to_string());
        assert_eq!(
            "Single:Double",
            Path(vec!["Single".to_string(), "Double".to_string()]).to_string()
        );
        assert_eq!(
            "Single:Double:Triple",
            Path(vec![
                "Single".to_string(),
                "Double".to_string(),
                "Triple".to_string()
            ])
            .to_string()
        );
    }
}
