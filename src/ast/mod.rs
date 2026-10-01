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
    /// The first day of the period containing `t`.
    pub fn period_start(self, t: chrono::NaiveDate) -> chrono::NaiveDate {
        use chrono::Datelike;
        let month = match self {
            AggKind::Ytd => 1,
            AggKind::Qtd => (t.month() - 1) / 3 * 3 + 1,
            AggKind::Mtd => t.month(),
        };
        chrono::NaiveDate::from_ymd_opt(t.year(), month, 1).expect("valid first of month")
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
