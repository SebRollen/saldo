//! What each part of a model reads, once its names are resolved, its modules
//! made and its scenarios applied: which values, accounts and tags, and
//! where each is named, with every number written in it. For tools that show
//! how a model's parts depend on each other, or that change a number where
//! it's written.
//!
//! Spans are byte ranges of the model's text, for a model read with
//! [`crate::load`]. A module made with `mod x = f(…)` has its own values and
//! entries, whose spans are in the body of `f`, except where an argument
//! stands for a parameter: that's the argument, where the call is.

use crate::ast::{BinOp, EntryStmt, Expr, Path, Span, SpannedExpr};
use crate::resolver::{self, ParamBody};
use chrono::NaiveDate;

/// A model's values, entries and checks, as its scenarios have them.
#[derive(Clone, Debug, PartialEq)]
pub struct Graph {
    /// The accounts money moves into or out of, ordered by path.
    pub accounts: Vec<Account>,
    /// The values, each after the values it reads. Sets of days aren't
    /// values, and aren't here.
    pub values: Vec<Value>,
    pub entries: Vec<Entry>,
    pub checks: Vec<Expression>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Account {
    pub path: Path,
    /// Its opening balance, and the day it opens, if a `balances` block
    /// gives it one.
    pub opening: Option<(NaiveDate, Expression)>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Value {
    /// Its full name, like `loans.rate`.
    pub name: String,
    pub body: Body,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Body {
    /// `let x = …`.
    Expr(Expression),
    /// `let x { … }`: arms, of which the first that covers a day gives the
    /// value.
    Arms(Vec<Arm>),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Arm {
    /// All of it, from its days to its growth.
    pub span: Span,
    pub value: Expression,
    /// What it compounds at or adds each step, if it grows.
    pub growth: Option<Expression>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    /// Its full name, like `loans.car`.
    pub name: String,
    pub label: String,
    /// All of it, from `entry`.
    pub span: Span,
    /// Where the days it fires on are written, unless it fires every day.
    pub schedule: Option<Span>,
    /// The entries it fires after, by full name.
    pub after: Vec<String>,
    /// Its `let`s, in the order they're written, those in branches too.
    pub lets: Vec<Let>,
    /// Its moves, in the order they're written, a `for` loop's once for
    /// each account it goes through.
    pub moves: Vec<Move>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Let {
    pub name: String,
    pub value: Expression,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Move {
    pub from: Path,
    pub to: Path,
    pub amount: Expression,
    pub tag: Option<String>,
    /// What decides whether it moves: the condition of each `if` it's in,
    /// outermost first, and of each branch before its own.
    pub conditions: Vec<Condition>,
    /// All of it, from `from`.
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Condition {
    pub expr: Expression,
    /// Whether it holds when the move moves: a branch's own condition does,
    /// and those of the branches before it don't.
    pub holds: bool,
}

/// An expression, with what's named and written in it.
#[derive(Clone, Debug, PartialEq)]
pub struct Expression {
    pub span: Span,
    /// In the order they're written.
    pub refs: Vec<Ref>,
}

/// A name or a number in an expression.
#[derive(Clone, Debug, PartialEq)]
pub struct Ref {
    /// Where it's written: a name as it's written, or a number or bool with
    /// its `%` and its minus sign, if it has one of its own. Text that
    /// replaces a number's changes it and nothing else.
    pub span: Span,
    pub to: Target,
    pub reach: Reach,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Target {
    /// A number or a bool, as it's written.
    Literal,
    /// A value, by full name. A scenario's value that reads the one it
    /// replaces reads it by its name with `$` after it: that's the value as
    /// the model writes it.
    Value(String),
    /// An account's balance or a group's, or what has moved into one so
    /// far this period.
    Account(Path),
    /// One of an entry's `let`s.
    Local(String),
    /// What an entry's moves with a tag have moved so far this period.
    Tag { entry: String, tag: String },
}

/// How directly an expression follows what's in it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Reach {
    /// It's the whole expression, with a unit at most: `rent/month`.
    Whole,
    /// Through arithmetic: `rent * 12`, `-rent`, `rent + fees`.
    Arithmetic,
    /// Inside a call, a draw, a comparison or an `if`.
    Inside,
}

pub(crate) fn graph(model: &resolver::Model) -> Graph {
    let accounts = model.accounts.iter().map(|(path, account)| Account {
        path: path.clone(),
        opening: account
            .opening
            .as_ref()
            .map(|(value, date)| (*date, expression(value))),
    });
    let values = model.params.iter().map(|(name, body)| Value {
        name: name.clone(),
        body: match body {
            ParamBody::Const(value) => Body::Expr(expression(value)),
            ParamBody::Arms(arms) => Body::Arms(
                arms.iter()
                    .map(|arm| Arm {
                        span: arm.span,
                        value: expression(&arm.value),
                        growth: arm.growth.as_ref().map(|growth| Expression {
                            span: growth.span,
                            ..expression(&growth.by)
                        }),
                    })
                    .collect(),
            ),
        },
    });
    let entries = model.entries.iter().map(|entry| {
        let (mut lets, mut moves) = (Vec::new(), Vec::new());
        statements(&entry.body, &mut Vec::new(), &mut lets, &mut moves);
        Entry {
            name: entry.name.clone(),
            label: entry.label.clone(),
            span: entry.span,
            schedule: entry.when,
            after: entry.after.iter().map(|(name, _)| name.clone()).collect(),
            lets,
            moves,
        }
    });
    Graph {
        accounts: accounts.collect(),
        values: values.collect(),
        entries: entries.collect(),
        checks: model.checks.iter().map(|(_, e)| expression(e)).collect(),
    }
}

/// Adds the `let`s and moves of `stmts` to `lets` and `moves`, in the order
/// they're written, a move with `conditions` and those of the branches it's
/// in.
fn statements(
    stmts: &[EntryStmt],
    conditions: &mut Vec<Condition>,
    lets: &mut Vec<Let>,
    moves: &mut Vec<Move>,
) {
    for stmt in stmts {
        match stmt {
            EntryStmt::Let { name, value } => lets.push(Let {
                name: name.0.clone(),
                value: expression(value),
            }),
            EntryStmt::Move(m) => moves.push(Move {
                from: m.from.0.clone(),
                to: m.to.0.clone(),
                amount: expression(&m.amount),
                tag: m.tag.as_ref().map(|(tag, _)| tag.clone()),
                conditions: conditions.clone(),
                span: m.span,
            }),
            EntryStmt::If {
                branches,
                otherwise,
            } => {
                let outer = conditions.len();
                for (cond, body) in branches {
                    conditions.push(Condition {
                        expr: expression(cond),
                        holds: true,
                    });
                    statements(body, conditions, lets, moves);
                    conditions.last_mut().expect("just pushed").holds = false;
                }
                statements(otherwise, conditions, lets, moves);
                conditions.truncate(outer);
            }
            // The resolver has replaced these with what they stand for.
            EntryStmt::For { body, .. } => statements(body, conditions, lets, moves),
        }
    }
}

fn expression(e: &SpannedExpr) -> Expression {
    let mut refs = Vec::new();
    collect(e, Reach::Whole, &mut refs);
    refs.sort_by_key(|r| r.span.start);
    Expression { span: e.1, refs }
}

/// Adds what's named and written in `e` to `refs`, each reached as directly
/// as `reach` at most.
fn collect(e: &SpannedExpr, reach: Reach, refs: &mut Vec<Ref>) {
    let (expr, span) = e;
    let mut add = |to| {
        refs.push(Ref {
            span: *span,
            to,
            reach,
        })
    };
    match expr.as_ref() {
        Expr::Num(_) | Expr::Bool(_) => add(Target::Literal),
        Expr::Neg(x) if matches!(x.0.as_ref(), Expr::Num(_)) => add(Target::Literal),
        Expr::Global(name) => add(Target::Value(name.clone())),
        Expr::Local(name) => add(Target::Local(name.clone())),
        Expr::Balance(path) | Expr::AccountTotal(path, _) => add(Target::Account(path.clone())),
        Expr::TagTotal { entry, tag, .. } => add(Target::Tag {
            entry: entry.clone(),
            tag: tag.clone(),
        }),
        // A unit, or a limit each period, says what the amount is per.
        Expr::Per(x, _) | Expr::Period(x, _) => collect(x, reach, refs),
        Expr::Neg(x) => collect(x, reach.max(Reach::Arithmetic), refs),
        Expr::Bin(a, op, b) => {
            let reach = match op {
                BinOp::Add | BinOp::Sub | BinOp::Mul | BinOp::Div | BinOp::Pow => {
                    reach.max(Reach::Arithmetic)
                }
                _ => Reach::Inside,
            };
            collect(a, reach, refs);
            collect(b, reach, refs);
        }
        Expr::Not(x) | Expr::Named(_, x) => collect(x, Reach::Inside, refs),
        Expr::If { cond, then, else_ } => {
            for x in [cond, then, else_] {
                collect(x, Reach::Inside, refs);
            }
        }
        Expr::Call(_, args) | Expr::Draw(_, args, _) => {
            for arg in args {
                collect(arg, Reach::Inside, refs);
            }
        }
        // Resolving replaces names, and sets of days aren't values.
        Expr::Ref(_) | Expr::Days(_) | Expr::Range { .. } => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MODEL: &str = "\
balances @ 2026-01-01 {
  assets.cash = 5_000
}

let salary = 95_000/year
let raise = -1.5%
let prices {
  2026.. => 1 compounding inflation every year
  _      => 1
}
let inflation = normal(3%, 1%)
let bonus = if salary > 50_000/year then salary * 10% else 0/year

fn half(x) { x / 2 }

fn loan(amount, rate) {
  mod {
    let owed = amount
    entry repay @ 15th {
      assets.cash -> liabilities.loan : half(owed * rate / 12)
    }
  }
}

mod car = loan(20_000, rate: 6%)

entry paycheck \"Paycheck\" @ last after car.repay {
  let net = salary * (1 - raise)
  income.salary -> assets.cash : net #pay
  if assets.cash > 10_000 {
    assets.cash -> assets.savings : 1_000
  } else if bonus > 0/year {
    assets.cash -> assets.savings : bonus
  } else {
    for a in assets.cash {
      a -> expenses.fees : 5 * prices
    }
  }
}

check assets.cash >= paycheck.pay[year] - 100_000
";

    fn graph() -> Graph {
        crate::load(MODEL).unwrap().graph()
    }

    fn text(span: Span) -> &'static str {
        &MODEL[span.into_range()]
    }

    /// An expression's refs as they're written, each with what it is and
    /// how directly it's read.
    fn refs(e: &Expression) -> Vec<String> {
        let refs = e.refs.iter().map(|r| {
            let to = match &r.to {
                Target::Literal => "#".to_string(),
                Target::Value(name) => format!("value {name}"),
                Target::Account(path) => format!("account {path}"),
                Target::Local(name) => format!("let {name}"),
                Target::Tag { entry, tag } => format!("tag {entry}.{tag}"),
            };
            format!("{} {to} {:?}", text(r.span), r.reach)
        });
        refs.collect()
    }

    fn value(graph: &Graph, name: &str) -> Value {
        let values = graph.values.iter();
        values.clone().find(|v| v.name == name).cloned().unwrap()
    }

    fn expr(value: &Value) -> &Expression {
        match &value.body {
            Body::Expr(e) => e,
            Body::Arms(_) => panic!("{} has arms", value.name),
        }
    }

    #[test]
    fn values_name_what_they_read() {
        let graph = graph();
        assert_eq!(refs(expr(&value(&graph, "salary"))), ["95_000 # Whole"]);
        assert_eq!(refs(expr(&value(&graph, "raise"))), ["-1.5% # Whole"]);
        assert_eq!(
            refs(expr(&value(&graph, "inflation"))),
            ["3% # Inside", "1% # Inside"]
        );
        assert_eq!(
            refs(expr(&value(&graph, "bonus"))),
            [
                "salary value salary Inside",
                "50_000 # Inside",
                "salary value salary Inside",
                "10% # Inside",
                "0 # Inside",
            ]
        );
        let Body::Arms(arms) = &value(&graph, "prices").body else {
            panic!("prices has arms");
        };
        assert_eq!(
            text(arms[0].span),
            "2026.. => 1 compounding inflation every year"
        );
        assert_eq!(refs(&arms[0].value), ["1 # Whole"]);
        let growth = arms[0].growth.as_ref().unwrap();
        assert_eq!(text(growth.span), "compounding inflation every year");
        assert_eq!(refs(growth), ["inflation value inflation Whole"]);
        // Values come after the values they read.
        let at = |name| graph.values.iter().position(|v| v.name == name);
        assert!(at("inflation") < at("prices"));
        assert!(at("salary") < at("bonus"));
    }

    #[test]
    fn a_module_made_by_a_call_reads_its_arguments_where_they_are() {
        let graph = graph();
        let owed = value(&graph, "car.owed");
        assert_eq!(refs(expr(&owed)), ["20_000 # Whole"]);
        let repay = graph
            .entries
            .iter()
            .find(|e| e.name == "car.repay")
            .unwrap();
        assert_eq!(
            refs(&repay.moves[0].amount),
            ["owed value car.owed Inside", "12 # Inside", "6% # Inside"]
        );
        // The argument is where the call is, outside the expression.
        let amount = &repay.moves[0].amount;
        assert!(amount.refs[2].span.start > amount.span.end);
        assert_eq!(text(repay.schedule.unwrap()), "15th");
    }

    #[test]
    fn moves_say_what_decides_them() {
        let graph = graph();
        let paycheck = graph.entries.iter().find(|e| e.name == "paycheck").unwrap();
        assert_eq!(paycheck.label, "Paycheck");
        assert_eq!(paycheck.after, ["car.repay"]);
        assert_eq!(text(paycheck.schedule.unwrap()), "last");
        assert_eq!(paycheck.lets[0].name, "net");
        assert_eq!(
            refs(&paycheck.lets[0].value),
            [
                "salary value salary Arithmetic",
                "1 # Arithmetic",
                "raise value raise Arithmetic",
            ]
        );
        let moves: Vec<String> = paycheck
            .moves
            .iter()
            .map(|m| {
                let conditions = m.conditions.iter().map(|c| {
                    let sign = if c.holds { "" } else { "!" };
                    format!("{sign}({})", text(c.expr.span))
                });
                let conditions: Vec<String> = conditions.collect();
                let tag = m
                    .tag
                    .as_deref()
                    .map(|t| format!(" #{t}"))
                    .unwrap_or_default();
                format!(
                    "{} -> {} : {}{tag} {}",
                    m.from,
                    m.to,
                    text(m.amount.span),
                    conditions.join(" ")
                )
            })
            .collect();
        assert_eq!(
            moves,
            [
                "income.salary -> assets.cash : net #pay ",
                "assets.cash -> assets.savings : 1_000 (assets.cash > 10_000)",
                "assets.cash -> assets.savings : bonus !(assets.cash > 10_000) (bonus > 0/year)",
                "assets.cash -> expenses.fees : 5 * prices !(assets.cash > 10_000) !(bonus > 0/year)",
            ]
        );
        assert_eq!(refs(&paycheck.moves[0].amount), ["net let net Whole"]);
        assert_eq!(
            refs(&paycheck.moves[1].conditions[0].expr),
            ["assets.cash account assets.cash Inside", "10_000 # Inside"]
        );
    }

    #[test]
    fn accounts_and_checks() {
        let graph = graph();
        let paths: Vec<String> = graph.accounts.iter().map(|a| a.path.to_string()).collect();
        assert_eq!(
            paths,
            [
                "assets.cash",
                "assets.savings",
                "expenses.fees",
                "income.salary",
                "liabilities.loan",
            ]
        );
        let (date, opening) = graph.accounts[0].opening.clone().unwrap();
        assert_eq!(date, NaiveDate::from_ymd_opt(2026, 1, 1).unwrap());
        assert_eq!(refs(&opening), ["5_000 # Whole"]);
        assert_eq!(
            refs(&graph.checks[0]),
            [
                "assets.cash account assets.cash Inside",
                "paycheck.pay[year] tag paycheck.pay Inside",
                "100_000 # Inside",
            ]
        );
    }

    #[test]
    fn transactions_say_what_each_move_moved() {
        let opts = crate::RunOpts {
            from: NaiveDate::from_ymd_opt(2026, 1, 1).unwrap(),
            to: NaiveDate::from_ymd_opt(2026, 1, 31).unwrap(),
            seed: 1,
            every: crate::TimeUnit::Month,
            series: Vec::new(),
        };
        let output = crate::load(MODEL).unwrap().simulate(&opts).unwrap();
        let paycheck = output
            .log
            .transactions
            .iter()
            .find(|t| t.label.as_ref() == "Paycheck");
        let paycheck = paycheck.unwrap();
        assert_eq!(paycheck.entry.as_deref(), Some("paycheck"));
        // The paycheck, and then, with no more than 10,000 in cash before
        // it, the bonus saved.
        let moved: Vec<(usize, String)> = (paycheck.moves.iter())
            .map(|(i, amount)| (*i, amount.to_string()))
            .collect();
        assert_eq!(
            moved,
            [(0, "8035.42".to_string()), (2, "791.67".to_string())]
        );
        let repay = output
            .log
            .transactions
            .iter()
            .find(|t| t.label.as_ref() == "car.repay");
        assert_eq!(repay.unwrap().entry.as_deref(), Some("car.repay"));
    }

    #[test]
    fn scenarios_change_what_values_read() {
        let model = format!("{MODEL}\nscenario flat {{\n  let salary = 90_000/year * prices\n}}\n");
        let model = crate::load(&model).unwrap();
        let flat = model.scenario("flat").unwrap().graph();
        let salary = value(&flat, "salary");
        let refs: Vec<&Target> = expr(&salary).refs.iter().map(|r| &r.to).collect();
        assert_eq!(
            refs,
            [&Target::Literal, &Target::Value("prices".to_string())]
        );
    }
}
