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
}

#[derive(Clone, Debug)]
pub enum Expr {
    Num(Decimal),
    Bool(bool),
    Ref(Path),
    Neg(SpannedExpr),
    Bin(SpannedExpr, BinOp, SpannedExpr),
    If {
        cond: SpannedExpr,
        then: SpannedExpr,
        else_: SpannedExpr,
    },
    Call(String, Vec<SpannedExpr>),
    /// `.ytd`/`.qtd`/`.mtd` aggregation. The optional first field is the entry
    /// qualifier (e.g. `paycheck.k401_contrib.ytd` has `Some("paycheck")`).
    /// Unqualified form (`k401_contrib.ytd`) is only valid inside the defining entry.
    ParamAgg(Option<String>, String, AggKind),
}

#[derive(Clone, Debug)]
pub enum ScheduleRef {
    Literal(Schedule),
    Named(String),
}

#[derive(Clone, Debug)]
pub struct Interval {
    pub from: NaiveDate,
    pub to: Option<NaiveDate>,
    pub value: SpannedExpr,
}

impl Interval {
    pub fn contains(&self, t: NaiveDate) -> bool {
        t >= self.from && self.to.map(|to| t < to).unwrap_or(true)
    }
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
}

#[derive(Clone, Debug)]
pub enum ParamBody {
    Const(SpannedExpr),
    Schedule(Vec<Interval>),
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
        #[allow(dead_code)]
        unit: Option<String>,
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
}

#[derive(Clone, Debug)]
pub struct Program {
    pub decls: Vec<Spanned<Decl>>,
}

#[cfg(test)]
mod tests {
    use super::*;

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
