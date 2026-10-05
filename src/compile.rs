//! Lowers a resolved `Model` into a form the simulator can run without looking
//! anything up by name: accounts, values, tags, functions and `let`s all
//! become indices.

use crate::ast::schedule::DaySet;
use crate::ast::{self, AggKind, BinOp, GrowthKind, Path, Schedule, Span, TimeUnit};
use crate::resolver::{Model, ParamBody as ModelParamBody};
use chrono::NaiveDate;
use rust_decimal::Decimal;
use std::collections::HashMap;
use std::sync::Arc;

pub struct Program {
    /// Accounts, ordered by path.
    pub accounts: Vec<Arc<Path>>,
    /// What each account's ledger balance is multiplied by to read it.
    pub signs: Vec<Decimal>,
    /// Opening balance and date of each account, by account index.
    pub openings: Vec<Option<(Expr, NaiveDate)>>,
    /// Values ordered so each comes after the values it reads.
    pub params: Vec<Param>,
    pub fns: Vec<Function>,
    pub entries: Vec<Entry>,
    pub checks: Vec<(Schedule, Expr)>,
    pub tags: Vec<Tag>,
    /// Each `fill` in a move. The units pass adds these.
    pub fills: Vec<Fill>,
}

/// A `fill` in one of `entry`'s moves, which tracks what the move has moved
/// in the current `unit`.
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
    /// Arms, of which the first that covers a day gives the value.
    Arms(Vec<Arm>),
}

pub struct Arm {
    pub when: DaySet,
    pub value: Expr,
    pub growth: Option<Growth>,
}

/// How an arm's value grows: a step every `every` from `anchor`, the arm's
/// start, compounding at or adding `by`.
pub struct Growth {
    pub kind: GrowthKind,
    pub by: Expr,
    pub every: TimeUnit,
    pub anchor: NaiveDate,
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
    pub body: Vec<EntryStmt>,
    /// How many `let`s it has.
    pub slots: usize,
    pub span: Span,
}

pub enum EntryStmt {
    Let {
        slot: usize,
        value: Expr,
    },
    Move(Move),
    /// The statements of the first branch whose condition holds, or of
    /// `otherwise`.
    If {
        branches: Vec<(Expr, Vec<EntryStmt>)>,
        otherwise: Vec<EntryStmt>,
    },
}

impl EntryStmt {
    /// Calls `f` on every statement in `stmts`, including those in branches,
    /// in the order they're written.
    pub fn visit<'a>(stmts: &'a [EntryStmt], f: &mut impl FnMut(&'a EntryStmt)) {
        for stmt in stmts {
            f(stmt);
            if let EntryStmt::If {
                branches,
                otherwise,
            } = stmt
            {
                for (_, body) in branches {
                    EntryStmt::visit(body, f);
                }
                EntryStmt::visit(otherwise, f);
            }
        }
    }
}

pub struct Move {
    pub from: usize,
    pub to: usize,
    pub from_span: Span,
    pub to_span: Span,
    pub amount: Expr,
    pub tag: Option<usize>,
    /// The `fill`s in `amount`, which count what this move moves.
    pub fills: Vec<usize>,
    pub span: Span,
}

/// A tag on the moves of an entry.
pub struct Tag {
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
    /// The balance of an account, or the sum of a group's, read as their kind
    /// reads by multiplying by `sign`.
    Balance {
        accounts: Vec<usize>,
        sign: Decimal,
        /// Whether it's a group, which counts accounts that haven't opened
        /// yet as zero.
        group: bool,
    },
    Param(usize),
    /// A function's parameter or `let`, or an entry's `let`.
    Local(usize),
    /// What moves with a tag moved so far this period.
    Total(usize, AggKind),
    /// What moved into accounts so far this period, read like `Balance`.
    AccountTotal {
        accounts: Vec<usize>,
        sign: Decimal,
        agg: AggKind,
    },
    Neg(Box<Expr>),
    Not(Box<Expr>),
    Bin(Box<Expr>, BinOp, Box<Expr>),
    If(Box<Expr>, Box<Expr>, Box<Expr>),
    Builtin(Builtin, Vec<Expr>),
    Call(usize, Vec<Expr>),
    /// `x / year`. The units pass converts `x` if it's a rate per another
    /// unit, so the value passes through unchanged.
    Per(Box<Expr>, TimeUnit),
    /// `x[year]`: an amount each period, like a yearly limit. The units pass
    /// converts `x` if it's a rate, and fills it where a move moves it.
    Limit(Box<Expr>, TimeUnit),
    /// The firing entry's share of an amount per period. The units pass adds
    /// these where moves use rates.
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

struct Names<'m> {
    accounts: Vec<&'m Path>,
    params: HashMap<&'m str, usize>,
    fns: HashMap<&'m str, usize>,
    tags: HashMap<(&'m str, &'m str), usize>,
}

/// Lowers `model`, which the resolver has already validated: every name
/// resolves, so failing to find one is a bug.
pub fn compile(model: &Model) -> Program {
    let mut tags = Vec::new();
    let mut tag_ids = HashMap::new();
    for (i, entry) in model.entries.iter().enumerate() {
        for tag in &entry.tags {
            tag_ids.insert((entry.name.as_str(), tag.as_str()), tags.len());
            tags.push(Tag {
                name: format!("{}.{tag}", entry.name),
                entry: i,
            });
        }
    }
    let names = Names {
        accounts: model.accounts.keys().collect(),
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
        tags: tag_ids,
    };

    Program {
        accounts: model.accounts.keys().cloned().map(Arc::new).collect(),
        signs: model
            .accounts
            .keys()
            .map(|p| p.account_kind().expect("accounts have kinds").sign())
            .collect(),
        openings: model
            .accounts
            .values()
            .map(|a| {
                a.opening
                    .as_ref()
                    .map(|(e, date)| (names.lower(e, &[]), *date))
            })
            .collect(),
        params: model
            .params
            .iter()
            .map(|(name, body)| Param {
                name: name.clone(),
                body: match body {
                    ModelParamBody::Const(e) => ParamBody::Const(names.lower(e, &[])),
                    ModelParamBody::Arms(arms) => ParamBody::Arms(
                        arms.iter()
                            .map(|arm| Arm {
                                when: arm.when.clone(),
                                value: names.lower(&arm.value, &[]),
                                growth: arm.growth.as_ref().map(|growth| Growth {
                                    kind: growth.kind,
                                    by: names.lower(&growth.by, &[]),
                                    every: growth.every,
                                    anchor: arm
                                        .when
                                        .first_day()
                                        .expect("the resolver checks a growing arm has a start"),
                                }),
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
                let mut slots = 0;
                let body = names.lower_entry(entry, &entry.body, &mut Vec::new(), &mut slots);
                Entry {
                    label: entry.label.as_str().into(),
                    schedule: entry.schedule.clone(),
                    body,
                    slots,
                    span: entry.span,
                }
            })
            .collect(),
        checks: model
            .checks
            .iter()
            .map(|(schedule, e)| (schedule.clone(), names.lower(e, &[])))
            .collect(),
        tags,
        fills: Vec::new(),
    }
}

impl Names<'_> {
    fn account(&self, path: &Path) -> usize {
        self.accounts
            .binary_search(&path)
            .expect("the resolver collects every account")
    }

    /// Lowers `stmts`, statements of `entry`, in which `locals` are the
    /// `let`s in scope. Each `let` gets the next of `slots`.
    fn lower_entry(
        &self,
        entry: &crate::resolver::EntryDef,
        stmts: &[ast::EntryStmt],
        locals: &mut Vec<(String, usize)>,
        slots: &mut usize,
    ) -> Vec<EntryStmt> {
        let mut body = Vec::new();
        for stmt in stmts {
            body.push(match stmt {
                ast::EntryStmt::Let { name, value } => {
                    let value = self.lower(value, locals);
                    let slot = *slots;
                    *slots += 1;
                    locals.push((name.0.clone(), slot));
                    EntryStmt::Let { slot, value }
                }
                ast::EntryStmt::Move(m) => EntryStmt::Move(Move {
                    from: self.account(&m.from.0),
                    to: self.account(&m.to.0),
                    from_span: m.from.1,
                    to_span: m.to.1,
                    amount: self.lower(&m.amount, locals),
                    tag: m
                        .tag
                        .as_ref()
                        .map(|(tag, _)| self.tags[&(entry.name.as_str(), tag.as_str())]),
                    fills: Vec::new(),
                    span: m.span,
                }),
                ast::EntryStmt::If {
                    branches,
                    otherwise,
                } => EntryStmt::If {
                    branches: branches
                        .iter()
                        .map(|(cond, body)| {
                            let cond = self.lower(cond, locals);
                            (
                                cond,
                                self.lower_entry(entry, body, &mut locals.clone(), slots),
                            )
                        })
                        .collect(),
                    otherwise: self.lower_entry(entry, otherwise, &mut locals.clone(), slots),
                },
                ast::EntryStmt::For { .. } => unreachable!("the resolver expands loops"),
            });
        }
        body
    }

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
                    let value = self.lower(value, &locals);
                    let slot = locals.len();
                    locals.push((name.clone(), slot));
                    body.push(Stmt::Let {
                        slot,
                        name: name.clone(),
                        value,
                    });
                }
                ast::Stmt::Return(e) => body.push(Stmt::Return(self.lower(e, &locals))),
                ast::Stmt::Mod(_) => unreachable!("functions that make modules aren't compiled"),
            }
        }
        Function {
            name: name.to_string(),
            arity: def.params.len(),
            slots: locals.len(),
            body,
        }
    }

    /// The accounts under `path`, which is one of them or a group of them.
    fn accounts_under(&self, path: &Path) -> (Vec<usize>, Decimal, bool) {
        let accounts: Vec<usize> = self
            .accounts
            .iter()
            .enumerate()
            .filter(|(_, account)| account.starts_with(path))
            .map(|(i, _)| i)
            .collect();
        let group = !(accounts.len() == 1 && self.accounts[accounts[0]] == path);
        let sign = path.account_kind().expect("accounts have kinds").sign();
        (accounts, sign, group)
    }

    /// Lowers `expr`, in which `locals` are the function's or entry's
    /// parameters and `let`s, innermost last.
    fn lower(&self, (expr, span): &ast::SpannedExpr, locals: &[(String, usize)]) -> Expr {
        let lower = |e| Box::new(self.lower(e, locals));
        let kind = match expr.as_ref() {
            ast::Expr::Num(n) => ExprKind::Num(*n),
            ast::Expr::Bool(b) => ExprKind::Bool(*b),
            ast::Expr::Global(name) => ExprKind::Param(self.params[name.as_str()]),
            ast::Expr::Local(name) => {
                let (_, slot) = locals
                    .iter()
                    .rev()
                    .find(|(n, _)| n == name)
                    .expect("resolver checked");
                ExprKind::Local(*slot)
            }
            ast::Expr::Balance(path) => {
                let (accounts, sign, group) = self.accounts_under(path);
                ExprKind::Balance {
                    accounts,
                    sign,
                    group,
                }
            }
            ast::Expr::TagTotal { entry, tag, agg } => {
                ExprKind::Total(self.tags[&(entry.as_str(), tag.as_str())], *agg)
            }
            ast::Expr::AccountTotal(path, agg) => {
                let (accounts, sign, _) = self.accounts_under(path);
                ExprKind::AccountTotal {
                    accounts,
                    sign,
                    agg: *agg,
                }
            }
            ast::Expr::Neg(x) => ExprKind::Neg(lower(x)),
            ast::Expr::Per(x, unit) => ExprKind::Per(lower(x), *unit),
            ast::Expr::Period(x, unit) => ExprKind::Limit(lower(x), *unit),
            ast::Expr::Not(x) => ExprKind::Not(lower(x)),
            ast::Expr::Bin(a, op, b) => ExprKind::Bin(lower(a), *op, lower(b)),
            ast::Expr::If { cond, then, else_ } => {
                ExprKind::If(lower(cond), lower(then), lower(else_))
            }
            ast::Expr::Call(callee, args) => {
                let args = args.iter().map(|a| self.lower(a, locals)).collect();
                let name = callee.join();
                match Builtin::from_name(&name) {
                    Some(builtin) => ExprKind::Builtin(builtin, args),
                    None => ExprKind::Call(self.fns[name.as_str()], args),
                }
            }
            ast::Expr::Ref(_)
            | ast::Expr::Days(_)
            | ast::Expr::Range { .. }
            | ast::Expr::Named(..) => unreachable!("the resolver replaces these"),
        };
        Expr { kind, span: *span }
    }
}
