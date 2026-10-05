//! Sets of days, and the schedules entries fire on.
//!
//! A [`DaySet`] is what a day-set expression like `15th | last` or
//! `2026-01-02.. & fri / 2` means: whether it covers a given day. A
//! [`Schedule`] is the set of days an entry fires on, split into the parts that
//! decide how a rate per period is shared between its firings.

use super::{BinOp, DayLit, Expr, Path, Span, SpannedExpr, TimeUnit};
use crate::errors::Diagnostic;
use chrono::{Datelike, NaiveDate};
use rust_decimal::Decimal;
use std::cell::RefCell;
use std::fmt;

/// The English ordinal suffix for `n` ("st" for 1, "nd" for 22, "th" for 13…).
pub fn ordinal_suffix(n: u8) -> &'static str {
    if matches!(n % 100, 11..=13) {
        return "th";
    }
    match n % 10 {
        1 => "st",
        2 => "nd",
        3 => "rd",
        _ => "th",
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
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
    pub fn from_name(name: &str) -> Option<Self> {
        Some(match name {
            "monday" | "mon" => Dow::Monday,
            "tuesday" | "tue" => Dow::Tuesday,
            "wednesday" | "wed" => Dow::Wednesday,
            "thursday" | "thu" => Dow::Thursday,
            "friday" | "fri" => Dow::Friday,
            "saturday" | "sat" => Dow::Saturday,
            "sunday" | "sun" => Dow::Sunday,
            "weekday" => Dow::Weekday,
            "weekend" => Dow::Weekend,
            _ => return None,
        })
    }

    pub fn matches(&self, t: NaiveDate) -> bool {
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

    /// The most days matching `self` that any month can contain.
    pub fn max_per_month(&self) -> u8 {
        match self {
            Dow::Weekday => 23,
            Dow::Weekend => 10,
            _ => 5,
        }
    }

    pub fn name(&self) -> &'static str {
        match self {
            Dow::Monday => "Monday",
            Dow::Tuesday => "Tuesday",
            Dow::Wednesday => "Wednesday",
            Dow::Thursday => "Thursday",
            Dow::Friday => "Friday",
            Dow::Saturday => "Saturday",
            Dow::Sunday => "Sunday",
            Dow::Weekday => "weekday",
            Dow::Weekend => "weekend day",
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
    pub fn from_name(name: &str) -> Option<Self> {
        Some(match name {
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
        })
    }

    pub fn matches(&self, t: NaiveDate) -> bool {
        *self as u32 == t.month()
    }

    /// The most days this month can have (29 for February).
    pub fn max_days(self) -> u8 {
        match self {
            Month::February => 29,
            Month::April | Month::June | Month::September | Month::November => 30,
            _ => 31,
        }
    }
}

/// A quarter's number, from `q1` to `q4`.
pub fn quarter_from_name(name: &str) -> Option<u32> {
    Some(match name {
        "q1" => 1,
        "q2" => 2,
        "q3" => 3,
        "q4" => 4,
        _ => return None,
    })
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Ordinal {
    /// The `n`th, from 1.
    Nth(u8),
    Last,
}

impl Ordinal {
    /// Whether `t` is this day of its month. Days past the end of a short month
    /// fall on its last day, so "the 30th" is Feb 28 (or 29).
    pub fn matches(&self, t: NaiveDate) -> bool {
        match self {
            Ordinal::Nth(n) => {
                t.day() == u32::from(*n)
                    || (u32::from(*n) > t.num_days_in_month().into() && is_month_end(t))
            }
            Ordinal::Last => is_month_end(t),
        }
    }

    /// Whether `t` is this `dow` of its month, like its second Monday.
    pub fn matches_weekday(&self, dow: Dow, t: NaiveDate) -> bool {
        if !dow.matches(t) {
            return false;
        }
        // Counting matching days (rather than weeks) also handles `weekday`
        // and `weekend`, which match several days a week.
        let matches_on = |day: u32| t.with_day(day).is_some_and(|d| dow.matches(d));
        match self {
            Ordinal::Nth(n) => {
                (1..=t.day()).filter(|&day| matches_on(day)).count() == usize::from(*n)
            }
            Ordinal::Last => !(t.day() + 1..=u32::from(t.num_days_in_month())).any(matches_on),
        }
    }
}

impl fmt::Display for Ordinal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Ordinal::Nth(n) => write!(f, "{n}{}", ordinal_suffix(*n)),
            Ordinal::Last => f.write_str("last"),
        }
    }
}

fn is_month_end(t: NaiveDate) -> bool {
    t.day() == u32::from(t.num_days_in_month())
}

/// A set of days.
#[derive(Clone, Debug, PartialEq)]
pub enum DaySet {
    All,
    /// From the first day up to, but not including, the second.
    Span(NaiveDate, Option<NaiveDate>),
    /// These months every year, a bit each with January in bit 0.
    Months(u16),
    Weekday(Dow),
    /// That day of every month.
    MonthDay(Ordinal),
    /// That weekday of every month, like its second Monday.
    NthWeekday(Ordinal, Dow),
    /// That day of a month, every year.
    YearDay(Month, Ordinal),
    Union(Vec<DaySet>),
    Inter(Vec<DaySet>),
    Diff(Box<DaySet>, Box<DaySet>),
    /// Every `n`th day of a set.
    Thin(Thin),
}

/// Every `every`th day of `inner`, counting from its first day on or after
/// `anchor`, and back from it before.
#[derive(Clone, Debug)]
pub struct Thin {
    inner: Box<DaySet>,
    every: u32,
    /// The start of the range the set is intersected with; `None` until the
    /// intersection is known.
    anchor: Option<NaiveDate>,
    /// Where `/` is written, for reporting a missing anchor.
    span: Span,
    cache: RefCell<ThinCache>,
}

impl PartialEq for Thin {
    fn eq(&self, other: &Self) -> bool {
        self.inner == other.inner && self.every == other.every && self.anchor == other.anchor
    }
}

/// The days of a [`Thin`]'s inner set found so far, so each is only looked for
/// once however often the simulation asks.
#[derive(Clone, Debug, Default)]
struct ThinCache {
    /// Days on or after the anchor, earliest first.
    after: Vec<NaiveDate>,
    /// The last day looked at after the anchor.
    after_until: Option<NaiveDate>,
    /// Days before the anchor, latest first.
    before: Vec<NaiveDate>,
    /// The earliest day looked at before the anchor.
    before_until: Option<NaiveDate>,
}

impl Thin {
    fn contains(&self, t: NaiveDate) -> bool {
        if !self.inner.contains(t) {
            return false;
        }
        let anchor = self.anchor.expect("anchors are set while lowering");
        let every = self.every as usize;
        let mut cache = self.cache.borrow_mut();
        if t >= anchor {
            let mut day = match cache.after_until {
                Some(until) => until.succ_opt(),
                None => Some(anchor),
            };
            while let Some(d) = day
                && d <= t
            {
                if self.inner.contains(d) {
                    cache.after.push(d);
                }
                cache.after_until = Some(d);
                day = d.succ_opt();
            }
            let index = cache.after.binary_search(&t).expect("t is in the set");
            index.is_multiple_of(every)
        } else {
            let mut day = match cache.before_until {
                Some(until) => until.pred_opt(),
                None => anchor.pred_opt(),
            };
            while let Some(d) = day
                && d >= t
            {
                if self.inner.contains(d) {
                    cache.before.push(d);
                }
                cache.before_until = Some(d);
                day = d.pred_opt();
            }
            let index = cache
                .before
                .binary_search_by(|d| t.cmp(d))
                .expect("t is in the set");
            (index + 1).is_multiple_of(every)
        }
    }
}

/// What part a set plays in a schedule.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Role {
    /// Days of particular years: when the schedule is active.
    Window,
    /// Months or quarters of every year: which ones it keeps.
    Filter,
    /// Days within weeks, months or years: when it fires.
    Rhythm,
}

impl DaySet {
    pub fn contains(&self, t: NaiveDate) -> bool {
        match self {
            DaySet::All => true,
            DaySet::Span(from, to) => t >= *from && to.is_none_or(|to| t < to),
            DaySet::Months(months) => months & (1 << t.month0()) != 0,
            DaySet::Weekday(dow) => dow.matches(t),
            DaySet::MonthDay(ordinal) => ordinal.matches(t),
            DaySet::NthWeekday(ordinal, dow) => ordinal.matches_weekday(*dow, t),
            DaySet::YearDay(month, ordinal) => month.matches(t) && ordinal.matches(t),
            DaySet::Union(sets) => sets.iter().any(|s| s.contains(t)),
            DaySet::Inter(sets) => sets.iter().all(|s| s.contains(t)),
            DaySet::Diff(a, b) => a.contains(t) && !b.contains(t),
            DaySet::Thin(thin) => thin.contains(t),
        }
    }

    fn role(&self) -> Role {
        let all_same = |sets: &mut dyn Iterator<Item = &DaySet>| {
            let roles: Vec<Role> = sets.map(DaySet::role).collect();
            match roles.first() {
                Some(&first) if roles.iter().all(|r| *r == first) => first,
                _ => Role::Rhythm,
            }
        };
        match self {
            DaySet::Span(..) => Role::Window,
            DaySet::Months(_) => Role::Filter,
            DaySet::All
            | DaySet::Weekday(_)
            | DaySet::MonthDay(_)
            | DaySet::NthWeekday(..)
            | DaySet::YearDay(..)
            | DaySet::Thin(_) => Role::Rhythm,
            DaySet::Union(sets) | DaySet::Inter(sets) => all_same(&mut sets.iter()),
            DaySet::Diff(a, b) => all_same(&mut [a.as_ref(), b.as_ref()].into_iter()),
        }
    }

    /// A day before which the set has no days, if it's bounded below by a
    /// range of dates.
    pub fn lower_bound(&self) -> Option<NaiveDate> {
        match self {
            DaySet::Span(from, _) => Some(*from),
            DaySet::Inter(sets) => sets.iter().filter_map(DaySet::lower_bound).max(),
            DaySet::Union(sets) => sets
                .iter()
                .map(DaySet::lower_bound)
                .collect::<Option<Vec<_>>>()?
                .into_iter()
                .min(),
            DaySet::Diff(a, _) => a.lower_bound(),
            DaySet::Thin(thin) => thin.inner.lower_bound(),
            _ => None,
        }
    }

    /// The first day the set covers, if it's bounded below: where a growing
    /// value starts counting its steps.
    pub fn first_day(&self) -> Option<NaiveDate> {
        let bound = self.lower_bound()?;
        // Within a few years of the bound, unless nothing's there.
        let first = bound.iter_days().take(4 * 366).find(|d| self.contains(*d));
        Some(first.unwrap_or(bound))
    }

    /// Gives every `/ n` in the set that doesn't have one yet `anchor` to
    /// count from.
    fn anchor(&mut self, anchor: NaiveDate) {
        match self {
            DaySet::Thin(thin) => {
                thin.inner.anchor(anchor);
                thin.anchor.get_or_insert(anchor);
            }
            DaySet::Union(sets) | DaySet::Inter(sets) => {
                sets.iter_mut().for_each(|s| s.anchor(anchor));
            }
            DaySet::Diff(a, b) => {
                a.anchor(anchor);
                b.anchor(anchor);
            }
            _ => {}
        }
    }

    /// The span of the first `/ n` in the set with nothing to count from.
    fn unanchored(&self) -> Option<Span> {
        match self {
            DaySet::Thin(thin) => thin
                .inner
                .unanchored()
                .or(thin.anchor.is_none().then_some(thin.span)),
            DaySet::Union(sets) | DaySet::Inter(sets) => sets.iter().find_map(DaySet::unanchored),
            DaySet::Diff(a, b) => a.unanchored().or_else(|| b.unanchored()),
            _ => None,
        }
    }

    /// Whether every day `self` covers is covered by one of `earlier`, if
    /// that can be worked out: only for sets made of ranges, months and
    /// quarters, which change only where a range starts or ends and at the
    /// start of a month.
    pub fn is_shadowed_by(&self, earlier: &[&DaySet]) -> Option<bool> {
        let sets = std::iter::once(self).chain(earlier.iter().copied());
        if !sets.clone().all(DaySet::is_calendar) {
            return None;
        }
        // Before the first edge, no range has started; from the last one on,
        // each has started or ended. So a year either side stands for every
        // year beyond.
        let mut edges = Vec::new();
        for set in sets {
            set.edges(&mut edges);
        }
        let any_year = NaiveDate::from_ymd_opt(2000, 1, 1).expect("a valid date");
        let first = edges.iter().min().copied().unwrap_or(any_year);
        let last = edges.iter().max().copied().unwrap_or(any_year);
        let year = chrono::Months::new(12);
        let start = month_start(first).checked_sub_months(year).unwrap_or(first);
        let end = month_start(last)
            .checked_add_months(year)
            .unwrap_or(NaiveDate::MAX);
        let mut days: Vec<NaiveDate> = std::iter::successors(Some(start), |t| {
            t.checked_add_months(chrono::Months::new(1))
        })
        .take_while(|t| *t <= end)
        .chain(edges)
        .collect();
        days.sort();
        days.dedup();
        Some(
            days.into_iter()
                .all(|t| !self.contains(t) || earlier.iter().any(|s| s.contains(t))),
        )
    }

    fn is_calendar(&self) -> bool {
        match self {
            DaySet::All | DaySet::Span(..) | DaySet::Months(_) => true,
            DaySet::Union(sets) | DaySet::Inter(sets) => sets.iter().all(DaySet::is_calendar),
            DaySet::Diff(a, b) => a.is_calendar() && b.is_calendar(),
            _ => false,
        }
    }

    /// The dates its ranges start and end on.
    fn edges(&self, out: &mut Vec<NaiveDate>) {
        match self {
            DaySet::Span(from, to) => out.extend(std::iter::once(*from).chain(*to)),
            DaySet::Union(sets) | DaySet::Inter(sets) => sets.iter().for_each(|s| s.edges(out)),
            DaySet::Diff(a, b) => {
                a.edges(out);
                b.edges(out);
            }
            _ => {}
        }
    }
}

/// Reads the set of days `expr` describes. `names` reads the sets that names
/// in it stand for, with [`lower_part`].
pub fn lower(
    expr: &SpannedExpr,
    names: &mut dyn FnMut(&Path, Span) -> Result<DaySet, Diagnostic>,
) -> Result<DaySet, Diagnostic> {
    let set = lower_part(expr, names)?;
    if let Some(span) = set.unanchored() {
        return Err(Diagnostic::new(
            span,
            "there's no date to count from: intersect this with a range that has a start, \
             like `2026-01-02.. & fri / 2`",
        ));
    }
    Ok(set)
}

/// Reads the set of days `expr` describes, as part of a larger one: its `/ n`
/// may still need a range to count from.
pub fn lower_part(
    (expr, span): &SpannedExpr,
    names: &mut dyn FnMut(&Path, Span) -> Result<DaySet, Diagnostic>,
) -> Result<DaySet, Diagnostic> {
    let span = *span;
    Ok(match expr.as_ref() {
        Expr::Days(lit) => match lit {
            DayLit::All => DaySet::All,
            DayLit::Weekday(dow) => DaySet::Weekday(*dow),
            DayLit::MonthDay(ordinal) => DaySet::MonthDay(*ordinal),
            DayLit::NthWeekday(ordinal, dow) => DaySet::NthWeekday(*ordinal, *dow),
            DayLit::YearDay(month, ordinal) => DaySet::YearDay(*month, *ordinal),
            DayLit::Month(_) | DayLit::Quarter(_) => {
                let (first, last) = point(expr, span)?.months().expect("a yearly point");
                DaySet::Months(month_mask(first, last))
            }
            DayLit::MonthYear(..) | DayLit::QuarterYear(..) | DayLit::Date(_) => {
                let (from, to) = point(expr, span)?.dates().expect("a point in time");
                DaySet::Span(from, Some(to))
            }
        },
        Expr::Num(_) => {
            let (from, to) = point(expr, span)?.dates().expect("a year");
            DaySet::Span(from, Some(to))
        }
        Expr::Range {
            from,
            to,
            inclusive,
        } => range(from, to.as_ref(), *inclusive, span)?,
        Expr::Bin(a, op @ (BinOp::Union | BinOp::Inter), b) => {
            let mut sets = Vec::new();
            for side in [a, b] {
                match (lower_part(side, names)?, op) {
                    (DaySet::Union(inner), BinOp::Union) | (DaySet::Inter(inner), BinOp::Inter) => {
                        sets.extend(inner)
                    }
                    (set, _) => sets.push(set),
                }
            }
            if *op == BinOp::Union {
                DaySet::Union(sets)
            } else {
                // Every `/ n` in it counts from where its ranges of dates start.
                let mut set = DaySet::Inter(sets);
                let windows = match &set {
                    DaySet::Inter(sets) => sets
                        .iter()
                        .filter(|s| s.role() == Role::Window)
                        .filter_map(DaySet::lower_bound)
                        .max(),
                    _ => unreachable!(),
                };
                if let Some(anchor) = windows {
                    set.anchor(anchor);
                }
                set
            }
        }
        Expr::Bin(a, BinOp::Sub, b) => DaySet::Diff(
            Box::new(lower_part(a, names)?),
            Box::new(lower_part(b, names)?),
        ),
        Expr::Bin(a, BinOp::Div, b) => {
            let inner = lower_part(a, names)?;
            let every = match b.0.as_ref() {
                Expr::Num(n) if n.fract().is_zero() && *n >= Decimal::ONE => {
                    u32::try_from(n.mantissa()).ok().filter(|_| n.scale() == 0)
                }
                _ => None,
            };
            let Some(every) = every else {
                return Err(Diagnostic::new(
                    b.1,
                    "expected how many to count, a whole number like `2`",
                ));
            };
            DaySet::Thin(Thin {
                inner: Box::new(inner),
                every,
                anchor: None,
                span,
                cache: RefCell::default(),
            })
        }
        Expr::Ref(path) => names(path, span)?,
        _ => {
            return Err(Diagnostic::new(
                span,
                "expected a set of days, like `fri`, `15th | last` or `2026..`",
            ));
        }
    })
}

/// One end of a range.
#[derive(Clone, Copy)]
enum Point {
    /// From the first day up to, but not including, the second.
    Dates(NaiveDate, NaiveDate),
    /// A first and last month, from 1 for January, every year.
    Months(u32, u32),
}

impl Point {
    fn dates(self) -> Option<(NaiveDate, NaiveDate)> {
        match self {
            Point::Dates(from, to) => Some((from, to)),
            Point::Months(..) => None,
        }
    }

    fn months(self) -> Option<(u32, u32)> {
        match self {
            Point::Months(first, last) => Some((first, last)),
            Point::Dates(..) => None,
        }
    }
}

fn point(expr: &Expr, span: Span) -> Result<Point, Diagnostic> {
    let first_of = |year: i32, month: u32| NaiveDate::from_ymd_opt(year, month, 1);
    let months = |year: i32, first: u32, last: u32| {
        let end = if last == 12 {
            first_of(year + 1, 1)
        } else {
            first_of(year, last + 1)
        };
        Some(Point::Dates(first_of(year, first)?, end?))
    };
    let point = match expr {
        Expr::Num(n) => {
            let year = (n.fract().is_zero() && n.scale() == 0)
                .then(|| i32::try_from(n.mantissa()).ok())
                .flatten()
                .filter(|y| (1000..=9999).contains(y));
            year.and_then(|y| months(y, 1, 12))
        }
        Expr::Days(DayLit::Date(d)) => d.succ_opt().map(|next| Point::Dates(*d, next)),
        Expr::Days(DayLit::MonthYear(m, y)) => months(*y, *m as u32, *m as u32),
        Expr::Days(DayLit::QuarterYear(q, y)) => months(*y, q * 3 - 2, q * 3),
        Expr::Days(DayLit::Month(m)) => Some(Point::Months(*m as u32, *m as u32)),
        Expr::Days(DayLit::Quarter(q)) => Some(Point::Months(q * 3 - 2, q * 3)),
        _ => {
            return Err(Diagnostic::new(
                span,
                "a range runs between dates, years written in full like `2026`, \
                 months or quarters",
            ));
        }
    };
    point.ok_or_else(|| Diagnostic::new(span, "expected a year written in full, like `2026`"))
}

fn range(
    from: &SpannedExpr,
    to: Option<&SpannedExpr>,
    inclusive: bool,
    span: Span,
) -> Result<DaySet, Diagnostic> {
    let start = point(&from.0, from.1)?;
    let Some(to) = to else {
        return match start {
            Point::Dates(from, _) => Ok(DaySet::Span(from, None)),
            Point::Months(..) => Err(Diagnostic::new(
                span,
                "a range of months needs an end, like `sep..=dec`",
            )),
        };
    };
    match (start, point(&to.0, to.1)?) {
        (Point::Dates(from, _), Point::Dates(next, after)) => {
            let to = if inclusive { after } else { next };
            if to <= from {
                return Err(Diagnostic::new(span, "this ends before it starts"));
            }
            Ok(DaySet::Span(from, Some(to)))
        }
        (Point::Months(first, _), Point::Months(next, after)) => {
            let last = if inclusive {
                after
            } else if next == first {
                return Err(Diagnostic::new(
                    span,
                    "this has no months; write `..=` to include the last one",
                ));
            } else {
                (next + 10) % 12 + 1
            };
            Ok(DaySet::Months(month_mask(first, last)))
        }
        _ => Err(Diagnostic::new(
            span,
            "both ends of a range need a year, or neither",
        )),
    }
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

fn month_start(t: NaiveDate) -> NaiveDate {
    t.with_day(1).expect("every month has a first day")
}

/// The days an entry fires on, or a check is checked on.
///
/// A rate per period is shared between the days in each period that fit the
/// schedule's *rhythm* and *filter*: its days of the week, month or year, and
/// the months or quarters it keeps. The *window*, its ranges of dates and
/// years, only says when it's active, so a schedule that starts partway
/// through a year still gets each firing's share of a yearly amount, and so
/// only part of it.
#[derive(Clone, Debug, PartialEq)]
pub struct Schedule {
    rhythm: DaySet,
    filter: Option<DaySet>,
    window: Option<DaySet>,
}

impl Schedule {
    pub fn every_day() -> Self {
        Schedule::new(DaySet::All)
    }

    pub fn new(set: DaySet) -> Self {
        let parts = match set {
            DaySet::Inter(sets) => sets,
            set => vec![set],
        };
        let (mut rhythm, mut filter, mut window) = (Vec::new(), Vec::new(), Vec::new());
        for part in parts {
            match part.role() {
                Role::Rhythm => rhythm.push(part),
                Role::Filter => filter.push(part),
                Role::Window => window.push(part),
            }
        }
        let inter = |mut sets: Vec<DaySet>| match sets.len() {
            0 => None,
            1 => sets.pop(),
            _ => Some(DaySet::Inter(sets)),
        };
        // Dates alone, like `2026-04-15 | 2026-10-15`, are when it fires.
        if rhythm.is_empty() && filter.is_empty() {
            return Schedule {
                rhythm: inter(window).unwrap_or(DaySet::All),
                filter: None,
                window: None,
            };
        }
        Schedule {
            rhythm: inter(rhythm).unwrap_or(DaySet::All),
            filter: inter(filter),
            window: inter(window),
        }
    }

    pub fn matches(&self, t: NaiveDate) -> bool {
        self.matches_pattern(t) && self.window.as_ref().is_none_or(|w| w.contains(t))
    }

    /// Whether `t` fits the schedule's pattern, ignoring when it's active.
    pub fn matches_pattern(&self, t: NaiveDate) -> bool {
        self.rhythm.contains(t) && self.within(t)
    }

    /// Whether `t` is in a month or quarter the schedule keeps.
    fn within(&self, t: NaiveDate) -> bool {
        self.filter.as_ref().is_none_or(|f| f.contains(t))
    }

    /// Which part of an amount per `unit` belongs to the firing on `t`, which
    /// must fit the schedule.
    ///
    /// The days in `t`'s period that fit the schedule's pattern split the
    /// period's amount equally, including days before the schedule's window
    /// opens, so a schedule that starts partway through a period gets only
    /// part of it. Periods with no such days add their whole amount to the
    /// next day that fits, back to the start of the window (see
    /// `carried_periods`).
    pub fn share(&self, unit: TimeUnit, t: NaiveDate) -> Share {
        let start = unit.period_start(t);
        let mut share = Share {
            carried: 0,
            index: 0,
            count: 0,
        };
        for d in unit.days(start) {
            if self.matches_pattern(d) {
                share.count += 1;
                if d <= t {
                    share.index += 1;
                }
            }
        }
        share.carried = self
            .carried_periods(unit, t)
            .len()
            .try_into()
            .unwrap_or(u32::MAX);
        share
    }

    /// The first days of the periods before `t`'s that add their whole amount
    /// to the firing on `t`: those since the previous firing's, back to the
    /// start of the window, except the ones the filter leaves out and those
    /// outside the window.
    pub fn carried_periods(&self, unit: TimeUnit, t: NaiveDate) -> Vec<NaiveDate> {
        let own = unit.period_start(t);
        let after_previous = self
            .previous_match(own)
            .and_then(|d| unit.next_period_start(d));
        let from = self
            .window
            .as_ref()
            .and_then(DaySet::lower_bound)
            .map(|d| unit.period_start(d));
        let Some(first) = after_previous.into_iter().chain(from).max() else {
            return Vec::new();
        };
        std::iter::successors(Some(first), |p| unit.next_period_start(*p))
            .take_while(|p| *p < own)
            .filter(|p| !self.leaves_out(unit, *p))
            .collect()
    }

    /// Whether the period starting on `start` doesn't roll into the next
    /// firing: the filter keeps none of its days, or none of the days the
    /// schedule would have fired on in it, or it's outside the window.
    fn leaves_out(&self, unit: TimeUnit, start: NaiveDate) -> bool {
        if let Some(window) = &self.window
            && !unit.days(start).any(|d| window.contains(d))
        {
            return true;
        }
        let Some(filter) = &self.filter else {
            return false;
        };
        let (mut kept, mut fires, mut fires_kept) = (false, false, false);
        for d in unit.days(start) {
            let within = filter.contains(d);
            kept |= within;
            if self.rhythm.contains(d) {
                fires = true;
                fires_kept |= within;
            }
        }
        !kept || (fires && !fires_kept)
    }

    /// How many times the schedule fires from `t` to the end of `t`'s
    /// period, including on `t`.
    pub fn firings_left(&self, unit: TimeUnit, t: NaiveDate) -> u32 {
        let start = unit.period_start(t);
        std::iter::successors(Some(t), |d| d.succ_opt())
            .take_while(|d| unit.period_start(*d) == start)
            .filter(|d| self.matches(*d))
            .count()
            .try_into()
            .unwrap_or(u32::MAX)
    }

    /// The last day before `t` that fits the schedule's pattern, if there's one
    /// in the few centuries before it.
    fn previous_match(&self, t: NaiveDate) -> Option<NaiveDate> {
        // Long enough for the sparsest schedule, every 255th leap day.
        const MAX_DAYS: usize = 255 * 8 * 366;
        std::iter::successors(t.pred_opt(), NaiveDate::pred_opt)
            .take(MAX_DAYS)
            .find(|d| self.matches_pattern(*d))
    }
}

/// A firing's part of an amount per period: the `index`th of `count` equal
/// parts of its own period, plus, for the period's first firing, the whole
/// amounts of the `carried` periods before it that have no firing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Share {
    pub carried: u32,
    pub index: u32,
    pub count: u32,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn date(y: i32, m: u32, d: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, d).unwrap()
    }

    /// The set of days `src` describes.
    fn set(src: &str) -> DaySet {
        try_set(src).unwrap_or_else(|e| panic!("{src}: {}", e.message))
    }

    fn try_set(src: &str) -> Result<DaySet, Diagnostic> {
        let expr = crate::parser::parse_expression(src).expect("parses");
        lower(&expr, &mut |path, span| {
            Err(Diagnostic::new(span, format!("no names in tests: {path}")))
        })
    }

    /// The days from `from` to `to`, inclusive, that `src` covers.
    fn days(src: &str, from: NaiveDate, to: NaiveDate) -> Vec<NaiveDate> {
        let set = set(src);
        from.iter_days()
            .take_while(|d| *d <= to)
            .filter(|d| set.contains(*d))
            .collect()
    }

    mod sets {
        use super::*;

        #[test]
        fn weekdays() {
            let wed = set("wed");
            assert!(!wed.contains(date(2024, 12, 30))); // monday
            assert!(wed.contains(date(2025, 1, 1))); // wednesday
            let both = set("mon | wed");
            assert!(both.contains(date(2024, 12, 30)));
            assert!(!both.contains(date(2024, 12, 31)));
            assert!(both.contains(date(2025, 1, 1)));
            assert!(set("weekday").contains(date(2025, 1, 2))); // thursday
            assert!(!set("weekday").contains(date(2025, 1, 4))); // saturday
            assert!(set("weekend").contains(date(2025, 1, 5))); // sunday
        }

        #[test]
        fn days_of_the_month() {
            let last = set("last");
            assert!(last.contains(date(2024, 1, 31)));
            assert!(last.contains(date(2024, 2, 29)));
            assert!(!last.contains(date(2024, 3, 1)));
            // Days past the end of a short month fall on its last day.
            let thirtieth = set("30th");
            assert!(thirtieth.contains(date(2024, 1, 30)));
            assert!(!thirtieth.contains(date(2024, 1, 31)));
            assert!(thirtieth.contains(date(2024, 2, 29)));
            let semi = set("15th | last");
            assert!(semi.contains(date(2026, 3, 15)));
            assert!(semi.contains(date(2026, 3, 31)));
            assert!(!semi.contains(date(2026, 3, 16)));
        }

        #[test]
        fn weekdays_of_the_month() {
            let first = set("1st mon");
            assert!(first.contains(date(2024, 1, 1)));
            assert!(!first.contains(date(2024, 1, 8)));
            assert!(set("2nd mon").contains(date(2024, 1, 8)));
            let last = set("last mon");
            assert!(!last.contains(date(2024, 1, 22)));
            assert!(last.contains(date(2024, 1, 29)));
            // Ordinals count matching days, so `weekday` counts business days.
            let third = set("3rd weekday");
            assert!(third.contains(date(2025, 1, 3)));
            assert!(!third.contains(date(2025, 1, 6)));
            let last = set("last weekday");
            assert!(last.contains(date(2025, 1, 31)));
            assert!(!last.contains(date(2025, 3, 28)));
            assert!(last.contains(date(2025, 3, 31)));
        }

        #[test]
        fn days_of_the_year() {
            let leap = set("feb 29");
            assert!(leap.contains(date(2024, 2, 29)));
            assert!(!leap.contains(date(2024, 2, 28)));
            assert!(leap.contains(date(2025, 2, 28)));
            let end = set("feb last");
            assert!(!end.contains(date(2024, 2, 28)));
            assert!(end.contains(date(2025, 2, 28)));
            assert!(!end.contains(date(2025, 3, 31)));
            assert!(set("jan 1").contains(date(2030, 1, 1)));
            assert!(set("jan 1st").contains(date(2030, 1, 1)));
        }

        #[test]
        fn months_quarters_and_years() {
            assert!(set("jul").contains(date(2031, 7, 9)));
            assert!(!set("jul").contains(date(2031, 8, 9)));
            assert!(set("q2").contains(date(2026, 6, 30)));
            assert!(!set("q2").contains(date(2026, 7, 1)));
            assert!(set("2026").contains(date(2026, 12, 31)));
            assert!(!set("2026").contains(date(2027, 1, 1)));
            assert!(set("jul 2026").contains(date(2026, 7, 31)));
            assert!(!set("jul 2026").contains(date(2027, 7, 1)));
            assert!(set("q3 2027").contains(date(2027, 9, 30)));
            assert!(set("2026-04-15").contains(date(2026, 4, 15)));
            assert!(!set("2026-04-15").contains(date(2026, 4, 16)));
        }

        #[test]
        fn ranges() {
            // `..` leaves out its end and `..=` includes it.
            let to = set("2026-01-01..2026-04-01");
            assert!(to.contains(date(2026, 3, 31)));
            assert!(!to.contains(date(2026, 4, 1)));
            assert!(set("2026..2028").contains(date(2027, 12, 31)));
            assert!(!set("2026..2028").contains(date(2028, 1, 1)));
            assert!(set("2026..=2028").contains(date(2028, 12, 31)));
            assert!(set("jul 2027..").contains(date(2090, 1, 1)));
            assert!(!set("jul 2027..").contains(date(2027, 6, 30)));
            // Months wrap past December.
            let school = set("sep..=jun");
            assert!(school.contains(date(2026, 1, 15)));
            assert!(!school.contains(date(2026, 7, 15)));
            assert!(!set("sep..jun").contains(date(2026, 6, 15)));
            assert!(set("q1..=q2").contains(date(2026, 6, 15)));
        }

        #[test]
        fn range_errors() {
            let error = |src: &str| try_set(src).unwrap_err().message;
            assert_eq!(
                error("2026..jan"),
                "both ends of a range need a year, or neither"
            );
            assert_eq!(error("2027-01-01..2026"), "this ends before it starts");
            assert_eq!(
                error("sep.."),
                "a range of months needs an end, like `sep..=dec`"
            );
            assert!(error("26..").contains("year written in full"));
            assert!(error("fri..").contains("a range runs between"));
        }

        #[test]
        fn operators() {
            let from = date(2026, 1, 1);
            let to = date(2026, 1, 31);
            assert_eq!(
                days("(mon | fri) & 2026-01-10..", from, to),
                [12, 16, 19, 23, 26, 30].map(|d| date(2026, 1, d))
            );
            assert_eq!(
                days("weekday - fri & 2026-01-26..", from, to),
                [26, 27, 28, 29].map(|d| date(2026, 1, d))
            );
            assert_eq!(
                days("last - dec 31", date(2026, 11, 1), date(2026, 12, 31)),
                [date(2026, 11, 30)]
            );
        }

        #[test]
        fn every_nth_counts_from_the_start_of_its_range() {
            let from = date(2026, 1, 1);
            let to = date(2026, 1, 31);
            assert_eq!(
                days("2026-01-02.. & fri / 2", from, to),
                [2, 16, 30].map(|d| date(2026, 1, d))
            );
            // The first Friday on or after a Thursday start.
            assert_eq!(
                days("2026-01-08.. & fri / 2", from, to),
                [9, 23].map(|d| date(2026, 1, d))
            );
            assert_eq!(
                days("2026-01-01.. & * / 3", from, date(2026, 1, 10)),
                [1, 4, 7, 10].map(|d| date(2026, 1, d))
            );
            assert_eq!(
                days("2026-01-01.. & last / 3", from, date(2026, 12, 31)),
                [
                    date(2026, 1, 31),
                    date(2026, 4, 30),
                    date(2026, 7, 31),
                    date(2026, 10, 31)
                ]
            );
            // Counting carries on through the days the rest of the set leaves out.
            assert_eq!(
                days("2026-01-01.. & * / 2 & (jan 1 | jan 4 | jan 5)", from, to),
                [date(2026, 1, 1), date(2026, 1, 5)]
            );
            // Before its start, a set still counts back from it.
            let fridays = set("2026-01-01.. & fri / 2");
            let DaySet::Inter(parts) = &fridays else {
                panic!("an intersection")
            };
            assert!(parts[1].contains(date(2025, 12, 19)));
            assert!(!parts[1].contains(date(2025, 12, 26)));
        }

        #[test]
        fn every_nth_needs_a_start() {
            let error = try_set("fri / 2").unwrap_err();
            assert!(
                error.message.contains("no date to count from"),
                "{}",
                error.message
            );
            assert!(try_set("2026.. & fri / 0").is_err());
        }

        #[test]
        fn first_day() {
            assert_eq!(set("jul 2026..").first_day(), Some(date(2026, 7, 1)));
            assert_eq!(set("2026").first_day(), Some(date(2026, 1, 1)));
            assert_eq!(set("jul & 2026..").first_day(), Some(date(2026, 7, 1)));
            assert_eq!(set("2026 | 2028").first_day(), Some(date(2026, 1, 1)));
            assert_eq!(set("jul").first_day(), None);
            assert_eq!(set("jul | 2028").first_day(), None);
        }

        #[test]
        fn shadowed_when_the_sets_above_cover_every_day() {
            let shadowed = |src: &str, above: &[&str]| {
                let above: Vec<DaySet> = above.iter().map(|s| set(s)).collect();
                set(src).is_shadowed_by(&above.iter().collect::<Vec<_>>())
            };
            assert_eq!(shadowed("may", &["q2"]), Some(true));
            assert_eq!(shadowed("q2", &["may"]), Some(false));
            assert_eq!(shadowed("2025-06-01..", &["2025-01-01.."]), Some(true));
            assert_eq!(shadowed("2024-06-01..", &["2025-01-01.."]), Some(false));
            assert_eq!(shadowed("*", &["jan..=dec"]), Some(true));
            assert_eq!(shadowed("*", &["q2"]), Some(false));
            // May recurs before and after the range, so the range can't cover it.
            assert_eq!(shadowed("may", &["2026..2028"]), Some(false));
            assert_eq!(shadowed("may", &["2027.."]), Some(false));
            // A range is covered by months together with other ranges.
            assert_eq!(shadowed("q2 2027", &["may 2027", "jun 2027"]), Some(false));
            assert_eq!(
                shadowed("q2 2027", &["may 2027", "jun 2027", "apr"]),
                Some(true)
            );
            // Days of the week can't be worked out this way.
            assert_eq!(shadowed("fri", &["*"]), None);
        }
    }

    mod share {
        use super::*;

        fn schedule(src: &str) -> Schedule {
            Schedule::new(set(src))
        }

        /// The shares of the firings of `src` from `from` to `to`, inclusive.
        fn shares(src: &str, unit: TimeUnit, from: NaiveDate, to: NaiveDate) -> Vec<Share> {
            let schedule = schedule(src);
            from.iter_days()
                .take_while(|d| *d <= to)
                .filter(|d| schedule.matches(*d))
                .map(|d| schedule.share(unit, d))
                .collect()
        }

        fn share(carried: u32, index: u32, count: u32) -> Share {
            Share {
                carried,
                index,
                count,
            }
        }

        #[test]
        fn semi_monthly_splits_a_year_in_24() {
            let year = shares(
                "15th | last",
                TimeUnit::Year,
                date(2026, 1, 1),
                date(2026, 12, 31),
            );
            let expected: Vec<Share> = (1..=24).map(|i| share(0, i, 24)).collect();
            assert_eq!(year, expected);
        }

        #[test]
        fn biweekly_years_have_26_or_27_paydays() {
            let src = "2021-01-01.. & fri / 2";
            let to = date(2022, 12, 31);
            let paydays = shares(src, TimeUnit::Year, date(2021, 1, 1), to);
            assert_eq!(paydays.len(), 27 + 26);
            assert_eq!(paydays[0], share(0, 1, 27)); // 2021-01-01
            assert_eq!(paydays[26], share(0, 27, 27)); // 2021-12-31
            assert_eq!(paydays[27], share(0, 1, 26));
        }

        #[test]
        fn paydays_before_the_start_still_count() {
            // Jan 1 fits the pattern, so the year still has 27 paydays.
            let paydays = shares(
                "2021-01-15.. & fri / 2",
                TimeUnit::Year,
                date(2021, 1, 1),
                date(2021, 12, 31),
            );
            assert_eq!(paydays.len(), 26);
            assert_eq!(paydays[0], share(0, 2, 27));
        }

        #[test]
        fn a_schedule_starting_midyear_gets_part_of_the_year() {
            for src in ["2026-07-01.. & last", "jul 2026.. & last"] {
                let months = shares(src, TimeUnit::Year, date(2026, 1, 1), date(2026, 12, 31));
                assert_eq!(months.len(), 6, "{src}");
                assert_eq!(months[0], share(0, 7, 12), "{src}");
            }
        }

        #[test]
        fn every_other_friday_extends_back_before_a_thursday_start() {
            let schedule = schedule("2026-01-01.. & fri / 2");
            assert!(schedule.matches(date(2026, 1, 2)));
            assert!(schedule.matches_pattern(date(2025, 12, 19)));
            assert!(!schedule.matches_pattern(date(2025, 12, 26)));
            assert!(!schedule.matches(date(2025, 12, 19)));
        }

        #[test]
        fn periods_without_firings_carry_into_the_next() {
            let cases = [
                (
                    "mar 31 | jun 30 | sep 30 | dec 31",
                    TimeUnit::Month,
                    share(2, 1, 1),
                ),
                ("dec 31", TimeUnit::Month, share(11, 1, 1)),
                ("dec 31", TimeUnit::Quarter, share(3, 1, 1)),
                ("sun", TimeUnit::Day, share(6, 1, 1)),
                ("2020-01-01.. & dec 31 / 2", TimeUnit::Year, share(1, 1, 1)),
                ("2021-01-01.. & fri / 2", TimeUnit::Week, share(1, 1, 1)),
            ];
            for (src, unit, expected) in cases {
                let found = shares(src, unit, date(2026, 1, 1), date(2026, 12, 31));
                assert!(
                    found.iter().all(|s| *s == expected),
                    "{src} per {unit}: {found:?}"
                );
            }
        }

        #[test]
        fn carrying_starts_at_the_start_of_the_window() {
            let found = shares(
                "2026-03-17.. & last",
                TimeUnit::Day,
                date(2026, 3, 1),
                date(2026, 4, 30),
            );
            // Mar 17 to 31, then all of April.
            assert_eq!(found, [share(14, 1, 1), share(29, 1, 1)]);
            // Without a start, the first firing covers the whole month.
            let found = shares("last", TimeUnit::Day, date(2026, 3, 1), date(2026, 3, 31));
            assert_eq!(found, [share(30, 1, 1)]);
        }

        #[test]
        fn monthly_per_day_carries_the_rest_of_the_month() {
            let days: Vec<u32> = shares("last", TimeUnit::Day, date(2026, 1, 1), date(2026, 3, 31))
                .iter()
                .map(|s| s.carried + 1)
                .collect();
            assert_eq!(days, [31, 28, 31]);
        }

        #[test]
        fn months_left_out_dont_carry() {
            // A filter leaves July and August out.
            let found = shares(
                "1st & sep..=jun",
                TimeUnit::Month,
                date(2026, 6, 1),
                date(2026, 9, 30),
            );
            assert_eq!(found, [share(0, 1, 1), share(0, 1, 1)]);
            // So does a gap in the window.
            let found = shares(
                "dec 31 & (2026 | 2028)",
                TimeUnit::Year,
                date(2026, 1, 1),
                date(2028, 12, 31),
            );
            assert_eq!(found, [share(0, 1, 1), share(0, 1, 1)]);
        }

        #[test]
        fn firings_left_respect_the_window() {
            let schedule = schedule("2026-07-01.. & last");
            assert_eq!(schedule.firings_left(TimeUnit::Year, date(2026, 7, 31)), 6);
            assert_eq!(schedule.firings_left(TimeUnit::Year, date(2026, 12, 31)), 1);
            assert_eq!(
                schedule.firings_left(TimeUnit::Quarter, date(2026, 8, 31)),
                2
            );
        }

        #[test]
        fn date_lists_split_by_period() {
            let src = "2026-04-15 | 2026-06-15 | 2026-09-15 | 2027-01-15";
            let found = shares(src, TimeUnit::Year, date(2026, 1, 1), date(2027, 12, 31));
            assert_eq!(
                found,
                [
                    share(0, 1, 3),
                    share(0, 2, 3),
                    share(0, 3, 3),
                    share(0, 1, 1)
                ]
            );
            // The first date has no earlier one to carry from.
            let found = shares(src, TimeUnit::Month, date(2026, 1, 1), date(2026, 12, 31));
            assert_eq!(found, [share(0, 1, 1), share(1, 1, 1), share(2, 1, 1)]);
        }
    }
}
