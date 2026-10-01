//! Lowers a resolved `Model` into a form the simulator can run without looking
//! anything up by name: accounts, params, legs, functions and function locals
//! all become indices.

use crate::ast::Schedule;
use crate::ast::{self, AggKind, BinOp, Path, Span, TimeUnit};
use crate::resolver::Model;
use chrono::NaiveDate;
use rust_decimal::Decimal;
use std::collections::HashMap;
use std::sync::Arc;

pub struct Program {
    /// Accounts in declaration order.
    pub accounts: Vec<Arc<Path>>,
    /// Opening balance and date of each account, by account index.
    pub openings: Vec<Option<(Expr, NaiveDate)>>,
    /// Params ordered so each comes after the params it reads.
    pub params: Vec<Param>,
    pub fns: Vec<Function>,
    pub entries: Vec<Entry>,
    pub asserts: Vec<(Schedule, Expr)>,
    pub legs: Vec<Leg>,
    /// Each `fill` in a posting. The units pass adds these.
    pub fills: Vec<Fill>,
}

/// A `fill` in one of `entry`'s postings, which tracks what the posting has
/// posted in the current `unit`.
pub struct Fill {
    pub entry: usize,
    pub unit: TimeUnit,
    pub span: Span,
}

pub struct Param {
    pub name: String,
    pub body: ParamBody,
}

pub enum ParamBody {
    Const(Expr),
    Intervals(Vec<Interval>),
}

pub struct Interval {
    pub from: NaiveDate,
    pub to: Option<NaiveDate>,
    pub value: Expr,
}

impl Interval {
    pub fn contains(&self, t: NaiveDate) -> bool {
        t >= self.from && self.to.is_none_or(|to| t < to)
    }
}

pub struct Function {
    pub name: String,
    pub arity: usize,
    /// Parameters occupy the first `arity` slots, `let` bindings the rest.
    pub slots: usize,
    pub body: Vec<Stmt>,
}

pub enum Stmt {
    Let {
        slot: usize,
        name: String,
        value: Expr,
    },
    Return(Expr),
}

pub struct Entry {
    pub label: Arc<str>,
    pub schedule: Schedule,
    /// In evaluation order, with the auto-balanced posting (if any) last.
    pub postings: Vec<Posting>,
    pub span: Span,
}

pub struct Posting {
    pub account: usize,
    pub account_span: Span,
    pub amount: Amount,
    pub leg: Option<usize>,
    /// The `fill`s in `amount`, which count what this posting posts.
    pub fills: Vec<usize>,
}

pub enum Amount {
    Expr(Expr),
    /// Clear the account's balance.
    All,
    /// Balance the other postings.
    Auto,
}

pub struct Leg {
    pub name: String,
    pub entry: usize,
}

pub struct Expr {
    pub kind: ExprKind,
    pub span: Span,
}

pub enum ExprKind {
    Num(Decimal),
    Bool(bool),
    Account(usize),
    Param(usize),
    /// A leg of the entry being evaluated: its amount in the current firing.
    Leg(usize),
    /// A function parameter or `let` binding.
    Local(usize),
    /// A leg's period-to-date total.
    Total(usize, AggKind),
    Neg(Box<Expr>),
    Not(Box<Expr>),
    Bin(Box<Expr>, BinOp, Box<Expr>),
    If(Box<Expr>, Box<Expr>, Box<Expr>),
    Builtin(Builtin, Vec<Expr>),
    Call(usize, Vec<Expr>),
    /// `x per year`. The units pass converts `x` if it's a rate per another
    /// unit, so the value passes through unchanged.
    Per(Box<Expr>, TimeUnit),
    /// The firing entry's share of an amount per period. The units pass adds
    /// these where postings use rates.
    Spread(Box<Expr>, TimeUnit),
    /// `fill(x)`: what's left of `x` for the current period, split over the
    /// firings left in it. The units pass turns `fill` calls into these; the
    /// `usize` indexes `Program::fills`.
    Fill(Box<Expr>, TimeUnit, usize),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Builtin {
    Min,
    Max,
    Abs,
    Floor,
    Ceil,
    Round,
    Fill,
}

impl Builtin {
    fn from_name(name: &str) -> Option<Self> {
        Some(match name {
            "min" => Builtin::Min,
            "max" => Builtin::Max,
            "abs" => Builtin::Abs,
            "floor" => Builtin::Floor,
            "ceil" => Builtin::Ceil,
            "round" => Builtin::Round,
            "fill" => Builtin::Fill,
            _ => return None,
        })
    }

    pub fn name(self) -> &'static str {
        match self {
            Builtin::Min => "min",
            Builtin::Max => "max",
            Builtin::Abs => "abs",
            Builtin::Floor => "floor",
            Builtin::Ceil => "ceil",
            Builtin::Round => "round",
            Builtin::Fill => "fill",
        }
    }
}

/// Where names in an expression resolve.
enum Scope<'a> {
    /// Params, accounts, and — inside an entry — that entry's legs.
    Global { entry: Option<&'a str> },
    /// A function body's parameters and `let` bindings, innermost last.
    Function(&'a [(String, usize)]),
}

struct Names<'m> {
    accounts: HashMap<&'m Path, usize>,
    params: HashMap<&'m str, usize>,
    fns: HashMap<&'m str, usize>,
    legs: HashMap<(&'m str, &'m str), usize>,
}

/// Lowers `model`, which the resolver has already validated: every name
/// resolves, so failing to find one is a bug.
pub fn compile(model: &Model) -> Program {
    let mut legs = Vec::new();
    let mut leg_ids = HashMap::new();
    for (i, entry) in model.entries.iter().enumerate() {
        for leg in entry.postings.iter().filter_map(|p| p.leg_name.as_deref()) {
            leg_ids.insert((entry.key.as_str(), leg), legs.len());
            legs.push(Leg {
                name: leg.to_string(),
                entry: i,
            });
        }
    }
    let names = Names {
        accounts: model
            .stocks
            .keys()
            .enumerate()
            .map(|(i, p)| (p, i))
            .collect(),
        params: model
            .params
            .keys()
            .enumerate()
            .map(|(i, n)| (n.as_str(), i))
            .collect(),
        fns: model
            .fns
            .keys()
            .enumerate()
            .map(|(i, n)| (n.as_str(), i))
            .collect(),
        legs: leg_ids,
    };
    let global = Scope::Global { entry: None };

    Program {
        accounts: model.stocks.keys().cloned().map(Arc::new).collect(),
        openings: model
            .stocks
            .values()
            .map(|a| {
                a.opening
                    .as_ref()
                    .map(|(e, date)| (names.lower(e, &global), *date))
            })
            .collect(),
        params: model
            .params
            .iter()
            .map(|(name, body)| Param {
                name: name.clone(),
                body: match body {
                    ast::ParamBody::Const(e) => ParamBody::Const(names.lower(e, &global)),
                    ast::ParamBody::Schedule(intervals) => ParamBody::Intervals(
                        intervals
                            .iter()
                            .map(|iv| Interval {
                                from: iv.from,
                                to: iv.to,
                                value: names.lower(&iv.value, &global),
                            })
                            .collect(),
                    ),
                },
            })
            .collect(),
        fns: model
            .fns
            .iter()
            .map(|(name, def)| names.lower_fn(name, def))
            .collect(),
        entries: model
            .entries
            .iter()
            .map(|entry| {
                let scope = Scope::Global {
                    entry: Some(&entry.key),
                };
                Entry {
                    label: entry.label.as_str().into(),
                    schedule: entry.schedule.clone(),
                    postings: entry
                        .postings
                        .iter()
                        .map(|p| Posting {
                            account: names.accounts[&p.account],
                            account_span: p.account_span,
                            amount: match &p.amount {
                                Some(ast::PostingAmount::Expr(e)) => {
                                    Amount::Expr(names.lower(e, &scope))
                                }
                                Some(ast::PostingAmount::All) => Amount::All,
                                None => Amount::Auto,
                            },
                            leg: p
                                .leg_name
                                .as_deref()
                                .map(|leg| names.legs[&(entry.key.as_str(), leg)]),
                            fills: Vec::new(),
                        })
                        .collect(),
                    span: entry.span,
                }
            })
            .collect(),
        asserts: model
            .asserts
            .iter()
            .map(|(schedule, e)| (schedule.clone(), names.lower(e, &global)))
            .collect(),
        legs,
        fills: Vec::new(),
    }
}

impl Names<'_> {
    fn lower_fn(&self, name: &str, def: &crate::resolver::FnDef) -> Function {
        let mut locals: Vec<(String, usize)> = def
            .params
            .iter()
            .enumerate()
            .map(|(slot, p)| (p.clone(), slot))
            .collect();
        let mut body = Vec::new();
        for stmt in &def.body {
            match stmt {
                ast::Stmt::Let { name, value } => {
                    let value = self.lower(value, &Scope::Function(&locals));
                    let slot = locals.len();
                    locals.push((name.clone(), slot));
                    body.push(Stmt::Let {
                        slot,
                        name: name.clone(),
                        value,
                    });
                }
                ast::Stmt::Return(e) => {
                    body.push(Stmt::Return(self.lower(e, &Scope::Function(&locals))));
                }
            }
        }
        Function {
            name: name.to_string(),
            arity: def.params.len(),
            slots: locals.len(),
            body,
        }
    }

    fn lower(&self, (expr, span): &ast::SpannedExpr, scope: &Scope) -> Expr {
        let lower = |e| Box::new(self.lower(e, scope));
        let kind = match expr.as_ref() {
            ast::Expr::Num(n) => ExprKind::Num(*n),
            ast::Expr::Bool(b) => ExprKind::Bool(*b),
            ast::Expr::Ref(path) => self.lower_ref(path, scope),
            ast::Expr::Neg(x) => ExprKind::Neg(lower(x)),
            ast::Expr::Per(x, unit) => ExprKind::Per(lower(x), *unit),
            ast::Expr::Not(x) => ExprKind::Not(lower(x)),
            ast::Expr::Bin(a, op, b) => ExprKind::Bin(lower(a), *op, lower(b)),
            ast::Expr::If { cond, then, else_ } => {
                ExprKind::If(lower(cond), lower(then), lower(else_))
            }
            ast::Expr::Call(name, args) => {
                let args = args.iter().map(|a| self.lower(a, scope)).collect();
                match Builtin::from_name(name) {
                    Some(builtin) => ExprKind::Builtin(builtin, args),
                    None => ExprKind::Call(self.fns[name.as_str()], args),
                }
            }
            ast::Expr::ParamAgg(qualifier, leg, kind) => {
                let Scope::Global { entry } = scope else {
                    unreachable!("the resolver rejects aggregates in function bodies")
                };
                let key = qualifier.as_deref().or(*entry).expect("resolver checked");
                ExprKind::Total(self.legs[&(key, leg.as_str())], *kind)
            }
        };
        Expr { kind, span: *span }
    }

    fn lower_ref(&self, path: &Path, scope: &Scope) -> ExprKind {
        let single = (path.0.len() == 1).then(|| path.0[0].as_str());
        match scope {
            Scope::Function(locals) => {
                let name = single.expect("resolver checked");
                let (_, slot) = locals
                    .iter()
                    .rev()
                    .find(|(n, _)| n == name)
                    .expect("resolver checked");
                ExprKind::Local(*slot)
            }
            Scope::Global { entry } => {
                if let (Some(entry), Some(name)) = (entry, single)
                    && let Some(&leg) = self.legs.get(&(*entry, name))
                {
                    return ExprKind::Leg(leg);
                }
                if let Some(&account) = self.accounts.get(path) {
                    return ExprKind::Account(account);
                }
                ExprKind::Param(self.params[single.expect("resolver checked")])
            }
        }
    }
}
