pub mod schedule;

use chrono::NaiveDate;
use rust_decimal::Decimal;
pub use schedule::{Dow, Month, Ordinal, Schedule};

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

/// A dotted name, like `assets.cash` or `loans.car.repay`.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Path(pub Vec<String>);

impl Path {
    pub fn join(&self) -> String {
        self.0.join(".")
    }

    /// The account's name in ledger output: each part capitalized, with `_`
    /// starting a new word, so `liabilities.taxes_payable` is
    /// `Liabilities:TaxesPayable`.
    pub fn ledger_name(&self) -> String {
        self.0
            .iter()
            .map(|part| {
                part.split('_')
                    .map(|word| {
                        let mut chars = word.chars();
                        chars.next().map_or(String::new(), |first| {
                            first.to_uppercase().chain(chars).collect()
                        })
                    })
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join(":")
    }

    /// The kind of account this path names, if it starts with a kind's root.
    pub fn account_kind(&self) -> Option<AccountKind> {
        AccountKind::from_root(self.0.first()?)
    }

    /// Whether `self` is `other` or one of its descendants.
    pub fn starts_with(&self, other: &Path) -> bool {
        self.0.starts_with(&other.0)
    }
}

impl std::fmt::Display for Path {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.join())
    }
}

/// What an account holds, named by the first part of its path.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum AccountKind {
    Assets,
    Liabilities,
    Income,
    Expenses,
    Equity,
}

impl AccountKind {
    pub const ROOTS: [&'static str; 5] = ["assets", "liabilities", "income", "expenses", "equity"];

    pub fn from_root(root: &str) -> Option<Self> {
        Some(match root {
            "assets" => AccountKind::Assets,
            "liabilities" => AccountKind::Liabilities,
            "income" => AccountKind::Income,
            "expenses" => AccountKind::Expenses,
            "equity" => AccountKind::Equity,
            _ => return None,
        })
    }

    /// What a ledger balance is multiplied by to read it: assets and expenses
    /// read as they print, and liabilities, income and equity, which the
    /// ledger shows as negative, read positive.
    pub fn sign(self) -> Decimal {
        match self {
            AccountKind::Assets | AccountKind::Expenses => Decimal::ONE,
            AccountKind::Liabilities | AccountKind::Income | AccountKind::Equity => {
                Decimal::NEGATIVE_ONE
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum AggKind {
    Ytd,
    Qtd,
    Mtd,
}

impl AggKind {
    /// The total kept for `unit`, if there is one.
    pub fn from_unit(unit: TimeUnit) -> Option<Self> {
        match unit {
            TimeUnit::Year => Some(AggKind::Ytd),
            TimeUnit::Quarter => Some(AggKind::Qtd),
            TimeUnit::Month => Some(AggKind::Mtd),
            TimeUnit::Week | TimeUnit::Day => None,
        }
    }

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

    /// The first day of the period after the one containing `t`.
    pub fn next_period_start(self, t: NaiveDate) -> Option<NaiveDate> {
        let start = self.period_start(t);
        match self {
            TimeUnit::Day => start.succ_opt(),
            TimeUnit::Week => start.checked_add_days(chrono::Days::new(7)),
            TimeUnit::Month => start.checked_add_months(chrono::Months::new(1)),
            TimeUnit::Quarter => start.checked_add_months(chrono::Months::new(3)),
            TimeUnit::Year => start.checked_add_months(chrono::Months::new(12)),
        }
    }

    /// The date `n` of these after `anchor`, on its day of the month where
    /// the month has it and on the month's last day where it doesn't.
    pub fn after(self, anchor: NaiveDate, n: u32) -> Option<NaiveDate> {
        let months = |per: u32| anchor.checked_add_months(chrono::Months::new(n.checked_mul(per)?));
        match self {
            TimeUnit::Day => anchor.checked_add_days(chrono::Days::new(n.into())),
            TimeUnit::Week => anchor.checked_add_days(chrono::Days::new(u64::from(n) * 7)),
            TimeUnit::Month => months(1),
            TimeUnit::Quarter => months(3),
            TimeUnit::Year => months(12),
        }
    }

    /// How many of these have passed from `anchor` to `t`: the largest `n`
    /// with `after(anchor, n)` no later than `t`.
    pub fn steps(self, anchor: NaiveDate, t: NaiveDate) -> u32 {
        use chrono::Datelike;
        if t <= anchor {
            return 0;
        }
        let months = (i64::from(t.year()) - i64::from(anchor.year())) * 12 + i64::from(t.month())
            - i64::from(anchor.month());
        // Never fewer than the steps that have passed.
        let most = match self {
            TimeUnit::Day => (t - anchor).num_days(),
            TimeUnit::Week => (t - anchor).num_days() / 7,
            TimeUnit::Month => months,
            TimeUnit::Quarter => months / 3,
            TimeUnit::Year => months / 12,
        };
        let mut n = u32::try_from(most).unwrap_or(u32::MAX);
        while n > 0 && self.after(anchor, n).is_none_or(|d| d > t) {
            n -= 1;
        }
        n
    }

    /// The days of the period starting on `start`.
    pub fn days(self, start: NaiveDate) -> impl Iterator<Item = NaiveDate> {
        std::iter::successors(Some(start), |d| d.succ_opt())
            .take_while(move |d| self.period_start(*d) == start)
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
        write!(f, "[{}]", self.unit())
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
    /// `|`: the days in either set.
    Union,
    /// `&`: the days in both sets.
    Inter,
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
            BinOp::Union => "|",
            BinOp::Inter => "&",
        })
    }
}

#[derive(Clone, Debug)]
pub enum Expr {
    Num(Decimal),
    Bool(bool),
    /// A name as written. The resolver replaces these with what they name.
    Ref(Path),
    Neg(SpannedExpr),
    Not(SpannedExpr),
    Bin(SpannedExpr, BinOp, SpannedExpr),
    If {
        cond: SpannedExpr,
        then: SpannedExpr,
        else_: SpannedExpr,
    },
    /// A call. The resolver replaces a user function's path with its full name.
    Call(Path, Vec<SpannedExpr>),
    /// `x / year`: an amount per period.
    Per(SpannedExpr, TimeUnit),
    /// `x[year]`: a total so far this period.
    Period(SpannedExpr, TimeUnit),
    /// A set of days written as a literal, like `fri` or `jan 1`.
    Days(DayLit),
    /// `a..b`, `a..=b` or `a..`.
    Range {
        from: SpannedExpr,
        to: Option<SpannedExpr>,
        inclusive: bool,
    },

    // What the resolver replaces names with.
    /// A `let` at the top of a module, by full name.
    Global(String),
    /// A function's parameter or `let`, or an entry's `let`.
    Local(String),
    /// An account's balance, or a group's: the sum of the accounts under it.
    Balance(Path),
    /// What moves with a tag have moved so far this period.
    TagTotal {
        entry: String,
        tag: String,
        agg: AggKind,
    },
    /// What has moved into an account (or a group) so far this period.
    AccountTotal(Path, AggKind),
}

/// A set of days written as a literal.
#[derive(Clone, Debug, PartialEq)]
pub enum DayLit {
    /// `*`: every day.
    All,
    /// `fri`, `weekday`.
    Weekday(Dow),
    /// `15th`, `last`: that day of every month.
    MonthDay(Ordinal),
    /// `2nd mon`, `last fri`: that weekday of every month.
    NthWeekday(Ordinal, Dow),
    /// `jan`: every day of a month, every year.
    Month(Month),
    /// `jan 1`, `feb last`: a day of a month, every year.
    YearDay(Month, Ordinal),
    /// `jul 2026`.
    MonthYear(Month, i32),
    /// `q2`: every day of a quarter, every year.
    Quarter(u32),
    /// `q3 2027`.
    QuarterYear(u32, i32),
    Date(NaiveDate),
}

/// A time-varying value's value on the days `when` covers.
#[derive(Clone, Debug)]
pub struct Arm {
    /// `None` for `_`, which covers every day.
    pub when: Option<SpannedExpr>,
    pub value: SpannedExpr,
    pub growth: Option<Growth>,
    pub span: Span,
}

/// How an arm's value grows, a step every `every` from the arm's start:
/// `compounding 3% every year` or `adding 25 every year`.
#[derive(Clone, Debug)]
pub struct Growth {
    pub kind: GrowthKind,
    /// The rate it compounds at, or what it adds, each step.
    pub by: SpannedExpr,
    pub every: TimeUnit,
    pub span: Span,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum GrowthKind {
    Compounding,
    Adding,
}

impl GrowthKind {
    pub fn keyword(self) -> &'static str {
        match self {
            GrowthKind::Compounding => "compounding",
            GrowthKind::Adding => "adding",
        }
    }
}

#[derive(Clone, Debug)]
pub enum LetBody {
    Expr(SpannedExpr),
    /// Arms, of which the first that covers a day gives the value.
    Arms(Vec<Arm>),
}

/// A statement in a function body.
#[derive(Clone, Debug)]
pub enum Stmt {
    Let {
        name: String,
        value: SpannedExpr,
    },
    /// The final expression, which the function returns.
    Return(SpannedExpr),
}

/// `from -> to : amount #tag`.
#[derive(Clone, Debug)]
pub struct Move {
    pub from: Spanned<Path>,
    pub to: Spanned<Path>,
    pub amount: SpannedExpr,
    pub tag: Option<Spanned<String>>,
    pub span: Span,
}

#[derive(Clone, Debug)]
pub enum EntryStmt {
    Let {
        name: Spanned<String>,
        value: SpannedExpr,
    },
    Move(Move),
}

#[derive(Clone, Debug)]
pub enum ModBody {
    /// `mod x`: the declarations in another file, until loading reads them.
    File,
    Inline(Vec<Spanned<Decl>>),
}

#[derive(Clone, Debug)]
pub enum Decl {
    Let {
        name: Spanned<String>,
        body: LetBody,
    },
    Fn {
        name: Spanned<String>,
        params: Vec<Spanned<String>>,
        body: Vec<Stmt>,
    },
    Entry {
        name: Spanned<String>,
        label: Option<String>,
        /// The days it fires, or every day.
        when: Option<SpannedExpr>,
        /// The entries it fires after, on days they both fire.
        after: Vec<Spanned<Path>>,
        body: Vec<EntryStmt>,
    },
    Check {
        expr: SpannedExpr,
        /// The days it's checked, or every day.
        when: Option<SpannedExpr>,
    },
    Balances {
        date: NaiveDate,
        balances: Vec<(Spanned<Path>, SpannedExpr)>,
    },
    Mod {
        name: Spanned<String>,
        body: ModBody,
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
    fn counts_steps_from_an_anchor() {
        use TimeUnit::*;
        let jan_31 = date(2026, 1, 31);
        assert_eq!(Month.after(jan_31, 1), Some(date(2026, 2, 28)));
        assert_eq!(Month.after(jan_31, 2), Some(date(2026, 3, 31)));
        assert_eq!(Month.steps(jan_31, date(2026, 2, 27)), 0);
        assert_eq!(Month.steps(jan_31, date(2026, 2, 28)), 1);
        assert_eq!(Month.steps(jan_31, date(2026, 3, 30)), 1);
        assert_eq!(Month.steps(jan_31, date(2026, 3, 31)), 2);
        let jul_1 = date(2026, 7, 1);
        assert_eq!(Year.steps(jul_1, date(2027, 6, 30)), 0);
        assert_eq!(Year.steps(jul_1, date(2027, 7, 1)), 1);
        assert_eq!(Year.steps(jul_1, date(2030, 1, 1)), 3);
        assert_eq!(Quarter.steps(jul_1, date(2026, 12, 31)), 1);
        assert_eq!(Week.steps(jul_1, date(2026, 7, 14)), 1);
        assert_eq!(Day.steps(jul_1, date(2026, 6, 1)), 0);
    }

    #[test]
    fn steps_through_periods() {
        use TimeUnit::*;
        // A Thursday.
        let t = date(2026, 12, 31);
        assert_eq!(Day.next_period_start(t), Some(date(2027, 1, 1)));
        assert_eq!(Week.next_period_start(t), Some(date(2027, 1, 4)));
        assert_eq!(
            Month.next_period_start(date(2026, 1, 31)),
            Some(date(2026, 2, 1))
        );
        assert_eq!(
            Quarter.next_period_start(date(2026, 11, 15)),
            Some(date(2027, 1, 1))
        );
        assert_eq!(
            Year.next_period_start(date(2026, 2, 28)),
            Some(date(2027, 1, 1))
        );
        assert_eq!(Month.days(date(2026, 2, 1)).count(), 28);
        assert_eq!(Week.days(date(2026, 12, 28)).last(), Some(date(2027, 1, 3)));
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
        let path = |parts: &[&str]| Path(parts.iter().map(|p| p.to_string()).collect());
        assert_eq!(path(&["assets", "cash"]).to_string(), "assets.cash");
        assert_eq!(path(&["assets", "cash"]).ledger_name(), "Assets:Cash");
        assert_eq!(
            path(&["liabilities", "taxes_payable", "current_year"]).ledger_name(),
            "Liabilities:TaxesPayable:CurrentYear"
        );
        assert_eq!(path(&["assets", "HSA"]).ledger_name(), "Assets:HSA");
        assert_eq!(path(&["expenses", "café"]).ledger_name(), "Expenses:Café");
    }
}
