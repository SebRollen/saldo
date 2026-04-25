use chrono::{Datelike, NaiveDate};
use chumsky::span::SimpleSpan;
use rust_decimal::Decimal;

pub type Span = SimpleSpan<usize>;
pub type SpannedExpr = (Box<Expr>, Span);

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
    Le,
    Gt,
    Ge,
    Eq,
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
    /// `.ytd`/`.qtd`/`.mtd` aggregation. The optional first field is the flow
    /// qualifier (e.g. `paycheck.k401_contrib.ytd` has `Some("paycheck")`).
    /// Unqualified form (`k401_contrib.ytd`) is only valid inside the defining flow.
    ParamAgg(Option<String>, String, AggKind),
}

#[derive(Clone, Debug)]
pub enum ScheduleKind {
    Daily,
    Monthly,
    Quarterly,
    Yearly,
    On(NaiveDate),
}

impl ScheduleKind {
    pub fn matches(&self, t: NaiveDate) -> bool {
        fn days_in_month(year: i32, month: u32) -> u32 {
            let (following_year, following_month) = if month == 12 {
                (year + 1, 1)
            } else {
                (year, month + 1)
            };
            NaiveDate::from_ymd_opt(following_year, following_month, 1)
                .and_then(|d| d.pred_opt())
                .map(|d| d.day())
                .unwrap_or(28)
        }

        match self {
            ScheduleKind::Daily => true,
            ScheduleKind::Monthly => t.day() == days_in_month(t.year(), t.month()),
            ScheduleKind::Quarterly => {
                matches!(t.month(), 3 | 6 | 9 | 12) && t.day() == days_in_month(t.year(), t.month())
            }
            ScheduleKind::Yearly => t.month() == 12 && t.day() == 31,
            ScheduleKind::On(d) => t == *d,
        }
    }
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
        init: Option<SpannedExpr>,
    },
    Param {
        name: String,
        #[allow(dead_code)]
        unit: Option<String>,
        body: ParamBody,
    },
    Flow {
        label: String,
        alias: Option<String>,
        schedule: ScheduleKind,
        postings: Vec<Posting>,
    },
    Assert(ScheduleKind, SpannedExpr),
}

#[derive(Clone, Debug)]
pub struct Program {
    pub decls: Vec<(Decl, Span)>,
}
