//! Checks how rates and amounts combine, and turns rates into amounts where
//! entries post them.
//!
//! `/year` and the like make a value a rate. A move moves an amount, not a
//! rate, so wherever a move uses a rate as an amount, this pass wraps it in
//! [`ExprKind::Spread`] to give each firing its share (see
//! [`Schedule::share`](crate::ast::Schedule::share)). Anywhere else, mixing a
//! rate with an amount is an error, except with a total over the same period:
//! a per-year limit minus a `[year]` total is what's left of the limit.
//!
//! It also turns `fill(x)` calls into [`ExprKind::Fill`], giving each its own
//! slot in [`Program::fills`].
//!
//! A random draw is drawn once per period of its kind: once a month for an
//! amount per month, and each day for a plain number, unless a `/month` or
//! `[month]` applies to an expression it's written in. A rate or a limit can't
//! hold a draw that changes more often than its period, or the firings in a
//! period would each move part of a different draw.

use crate::ast::{BinOp, Dist, GrowthKind, Span, TimeUnit};
use crate::compile::{
    Builtin, Draw, EntryStmt, Expr, ExprKind, Fill, Function, Growth, Param, ParamBody, Program,
    Stmt,
};
use crate::errors::Diagnostic;
use std::collections::HashMap;
use std::fmt;

/// What a number measures, as far as time goes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum Kind {
    /// A plain number, like a literal, which takes on the kind of whatever
    /// it's combined with.
    Number,
    /// An amount, like a balance or what a move moves.
    Amount,
    /// A `[year]`, `[quarter]` or `[month]` total: the amount so far this
    /// period.
    Total(TimeUnit),
    /// An amount per period, like a salary `/year`, which is spread over the
    /// period.
    Rate(TimeUnit),
    /// An amount each period, like a yearly limit `[year]`, which isn't.
    Limit(TimeUnit),
}

impl fmt::Display for Kind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Kind::Number => f.write_str("a number"),
            Kind::Amount => f.write_str("an amount"),
            Kind::Total(unit) => write!(f, "a {unit}-to-date total"),
            Kind::Rate(unit) => write!(f, "an amount per {unit}"),
            Kind::Limit(unit) => write!(f, "an amount each {unit}"),
        }
    }
}

/// What combines two operands, for error messages.
#[derive(Clone, Copy)]
enum Combiner {
    Op(BinOp),
    Builtin(Builtin),
    If,
}

impl Combiner {
    /// Whether it picks one operand, rather than computing from both.
    fn picks(self) -> bool {
        !matches!(self, Combiner::Op(_))
    }
}

impl fmt::Display for Combiner {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Combiner::Op(op) => write!(f, "`{op}`"),
            Combiner::Builtin(builtin) => write!(f, "`{}`", builtin.name()),
            Combiner::If => f.write_str("this `if`"),
        }
    }
}

/// Where an expression appears.
#[derive(Clone, Copy)]
struct Cx<'a> {
    /// In an entry, where rates are spread over the entry's firings.
    posting: bool,
    /// The amount a move moves: the whole expression, or what `min`, `max`
    /// and `if` pick from it. Only these can `fill`.
    posted: bool,
    /// Kinds of the current function's or entry's parameters and `let`s.
    locals: &'a [Option<Kind>],
    /// How often the current entry's `let`s draw anew (see
    /// [`Checker::drawn`]).
    local_draws: &'a [Drawn],
    /// The unit of the innermost `/unit` or `[unit]` that applies to the
    /// expression, which plain numbers drawn in it are drawn once a unit of.
    per: Option<TimeUnit>,
}

const GLOBAL: Cx = Cx {
    posting: false,
    posted: false,
    locals: &[],
    local_draws: &[],
    per: None,
};

/// The shortest period over which a new draw changes an expression, and where
/// that draw is, or `None` if it doesn't draw.
type Drawn = Option<(TimeUnit, Span)>;

/// The one of `a` and `b` that changes more often.
fn more_often(a: Drawn, b: Drawn) -> Drawn {
    match (a, b) {
        (Some((x, _)), Some((y, _))) if y < x => b,
        (Some(_), _) => a,
        (None, _) => b,
    }
}

/// Checks every expression in `program`, spreading the rates that moves use.
pub fn check(program: &mut Program) -> Vec<Diagnostic> {
    let mut checker = Checker {
        fns: &mut program.fns,
        draws: &mut program.draws,
        params: Vec::with_capacity(program.params.len()),
        param_draws: Vec::with_capacity(program.params.len()),
        instances: HashMap::new(),
        calls: Vec::new(),
        posting: None,
        fills: Vec::new(),
        posting_fills: Vec::new(),
        diags: Vec::new(),
    };
    // Each param comes after the params it reads.
    for param in &mut program.params {
        let (kind, drawn) = checker.param(param);
        checker.params.push(kind);
        checker.param_draws.push(drawn);
    }
    for (expr, _) in program.openings.iter_mut().flatten() {
        if let Some(kind @ (Kind::Rate(_) | Kind::Limit(_))) = checker.kind(expr, GLOBAL) {
            checker.error(Diagnostic::new(
                expr.span,
                format!("an opening balance must be an amount, but this is {kind}"),
            ));
        }
    }
    for (index, entry) in program.entries.iter_mut().enumerate() {
        let mut locals: Vec<Option<Kind>> = vec![None; entry.slots];
        let mut local_draws: Vec<Drawn> = vec![None; entry.slots];
        checker.entry(index, &mut entry.body, &mut locals, &mut local_draws);
    }
    checker.posting = None;
    for so_far in &mut program.totals_so_far {
        if let Some(kind @ (Kind::Rate(_) | Kind::Limit(_))) =
            checker.kind(&mut so_far.value, GLOBAL)
        {
            checker.error(Diagnostic::new(
                so_far.value.span,
                format!("a total must be an amount, but this is {kind}"),
            ));
        }
    }
    for (_, e) in &mut program.checks {
        checker.kind(e, GLOBAL);
    }
    let Checker {
        mut diags, fills, ..
    } = checker;
    program.fills = fills;
    diags.sort_by_key(|d| d.span.start);
    diags
}

struct Checker<'p> {
    fns: &'p mut [Function],
    draws: &'p mut [Draw],
    /// Kinds of the params checked so far; `None` for a param with an error
    /// (already reported).
    params: Vec<Option<Kind>>,
    /// How often the params checked so far draw anew.
    param_draws: Vec<Drawn>,
    /// What each function returns, by function and argument kinds.
    instances: HashMap<(usize, Vec<Kind>), Option<Kind>>,
    /// The calls whose function bodies are being checked, innermost last.
    calls: Vec<(usize, Span)>,
    /// The entry whose move is being checked, and the move's tag.
    posting: Option<(usize, Option<usize>)>,
    fills: Vec<Fill>,
    /// The `fill`s found so far in the move being checked.
    posting_fills: Vec<usize>,
    diags: Vec<Diagnostic>,
}

impl Checker<'_> {
    /// Checks the statements of entry `index`, in which `locals` are the kinds
    /// of its `let`s.
    fn entry(
        &mut self,
        index: usize,
        stmts: &mut [EntryStmt],
        locals: &mut Vec<Option<Kind>>,
        local_draws: &mut Vec<Drawn>,
    ) {
        for stmt in stmts {
            match stmt {
                // A `let` can mix rates with amounts, as a move can, but what
                // it holds is only moved, and so spread, where a move uses it.
                EntryStmt::Let { slot, value } => {
                    self.posting = Some((index, None));
                    let cx = Cx {
                        posting: true,
                        posted: false,
                        locals,
                        local_draws,
                        per: None,
                    };
                    let kind = self.kind(value, cx);
                    let drawn = self.drawn(value, cx);
                    locals[*slot] = kind;
                    local_draws[*slot] = drawn;
                }
                EntryStmt::Move(m) => {
                    self.posting = Some((index, m.tag));
                    let cx = Cx {
                        posting: true,
                        posted: true,
                        locals,
                        local_draws,
                        per: None,
                    };
                    if let Some(kind) = self.kind(&mut m.amount, cx) {
                        self.settle(&mut m.amount, kind);
                    }
                    m.fills = std::mem::take(&mut self.posting_fills);
                }
                EntryStmt::If {
                    branches,
                    otherwise,
                } => {
                    for (cond, body) in branches {
                        self.posting = Some((index, None));
                        let cx = Cx {
                            posting: true,
                            posted: false,
                            locals,
                            local_draws,
                            per: None,
                        };
                        self.kind(cond, cx);
                        self.entry(index, body, locals, local_draws);
                    }
                    self.entry(index, otherwise, locals, local_draws);
                }
            }
        }
    }

    /// Reports `diag`, pointing at the calls that led to it.
    fn error(&mut self, mut diag: Diagnostic) {
        for &(f, span) in self.calls.iter().rev() {
            diag = diag.with_note(span, format!("in this call to `{}`", self.fns[f].name));
        }
        self.diags.push(diag);
    }

    /// The kind of `param`, and how often it draws anew, which this sets on
    /// it too.
    fn param(&mut self, param: &mut Param) -> (Option<Kind>, Drawn) {
        let kind = self.param_kind(param);
        let drawn = match &param.body {
            ParamBody::Const(e) => self.drawn(e, GLOBAL),
            ParamBody::Arms(arms) => arms
                .iter()
                .map(|arm| self.drawn(&arm.value, GLOBAL))
                .fold(None, more_often),
        };
        param.draws_every = drawn.map(|(every, _)| every);
        (kind, drawn)
    }

    fn param_kind(&mut self, param: &mut Param) -> Option<Kind> {
        let name = &param.name;
        match &mut param.body {
            ParamBody::Const(e) => self.kind(e, GLOBAL),
            ParamBody::Arms(arms) => {
                let found: Vec<(Option<Kind>, Span)> = arms
                    .iter_mut()
                    .map(|arm| (self.kind(&mut arm.value, GLOBAL), arm.value.span))
                    .collect();
                for (arm, (value, _)) in arms.iter_mut().zip(&found) {
                    if let Some(growth) = &mut arm.growth {
                        self.growth(growth, (*value)?)?;
                    }
                }
                // The param is whatever its arms agree on.
                let mut agreed = Kind::Number;
                let mut first: Option<(Kind, Span)> = None;
                for (found, span) in found {
                    let found = found?;
                    agreed = match (agreed, found) {
                        (k, Kind::Number) | (Kind::Number, k) => k,
                        _ if agreed == found => agreed,
                        (Kind::Amount | Kind::Total(_), Kind::Amount | Kind::Total(_)) => {
                            Kind::Amount
                        }
                        _ => {
                            let (first, first_span) = first.expect("an earlier arm set it");
                            self.error(
                                Diagnostic::new(
                                    span,
                                    format!(
                                        "`{name}` is {first} in one arm \
                                         but {found} in another"
                                    ),
                                )
                                .with_note(first_span, first.to_string())
                                .with_note(span, found.to_string()),
                            );
                            return None;
                        }
                    };
                    if first.is_none() && found != Kind::Number {
                        first = Some((found, span));
                    }
                }
                Some(agreed)
            }
        }
    }

    /// Checks that a growth rate is a plain number and that what `adding`
    /// adds is the same kind as the `value` it adds to.
    fn growth(&mut self, growth: &mut Growth, value: Kind) -> Option<()> {
        let by = self.kind(&mut growth.by, GLOBAL)?;
        let fits = match growth.kind {
            GrowthKind::Compounding => by == Kind::Number,
            GrowthKind::Adding => by == Kind::Number || by == value,
        };
        if !fits {
            let message = match growth.kind {
                GrowthKind::Compounding => {
                    format!("`compounding` takes a plain number, like `4%`, not {by}")
                }
                GrowthKind::Adding => format!("`adding` adds {by} to {value}"),
            };
            self.error(Diagnostic::new(growth.by.span, message));
            return None;
        }
        Some(())
    }

    /// The kind of `e`, or `None` if it has an error (already reported).
    fn kind(&mut self, e: &mut Expr, cx: Cx) -> Option<Kind> {
        let kind = self.kind_of(e, cx)?;
        if let Kind::Rate(unit) | Kind::Limit(unit) = kind
            && let Some((every, at)) = self.drawn(e, cx)
            && every < unit
        {
            let example = match kind {
                Kind::Limit(_) => format!("normal(800, 100)[{unit}]"),
                _ => format!("normal(800, 100)/{unit}"),
            };
            self.error(
                Diagnostic::new(
                    e.span,
                    format!("this is {kind}, but part of it is drawn anew every {every}"),
                )
                .with_note(at, format!("drawn every {every}"))
                .with_note(
                    e.span,
                    format!(
                        "{kind} has to be drawn once a {unit} or less often; \
                         write the distribution itself that way, like `{example}`"
                    ),
                ),
            );
            return None;
        }
        Some(kind)
    }

    /// How often `e` changes with a new draw, through the values and `let`s
    /// it reads too.
    fn drawn(&self, e: &Expr, cx: Cx) -> Drawn {
        let drawn = |e: &Expr| self.drawn(e, cx);
        match &e.kind {
            ExprKind::Draw(_, args, slot) => args.iter().map(drawn).fold(
                Some((self.draws[*slot].period, self.draws[*slot].span)),
                more_often,
            ),
            ExprKind::Param(param) => self.param_draws.get(*param).copied().flatten(),
            ExprKind::Local(slot) => cx.local_draws.get(*slot).copied().flatten(),
            ExprKind::Num(_)
            | ExprKind::Bool(_)
            | ExprKind::Balance { .. }
            | ExprKind::Total(..)
            | ExprKind::AccountTotal { .. } => None,
            ExprKind::Neg(x)
            | ExprKind::Not(x)
            | ExprKind::Per(x, _)
            | ExprKind::Limit(x, _)
            | ExprKind::Spread(x, _)
            | ExprKind::Fill(x, ..) => drawn(x),
            ExprKind::Bin(a, _, b) => more_often(drawn(a), drawn(b)),
            ExprKind::If(c, t, f) => more_often(more_often(drawn(c), drawn(t)), drawn(f)),
            ExprKind::Builtin(_, args) | ExprKind::Call(_, args) => {
                args.iter().map(drawn).fold(None, more_often)
            }
        }
    }

    /// The kind of `e`, without checking how often it draws.
    fn kind_of(&mut self, e: &mut Expr, cx: Cx) -> Option<Kind> {
        if let ExprKind::Builtin(Builtin::Fill, _) = e.kind {
            return self.fill(e, cx);
        }
        let span = e.span;
        // Operands are only what's posted if `min`, `max` or `if` picks them.
        let below = Cx {
            posted: false,
            ..cx
        };
        match &mut e.kind {
            ExprKind::Num(_) | ExprKind::Bool(_) => Some(Kind::Number),
            ExprKind::Balance { .. } | ExprKind::Spread(..) => Some(Kind::Amount),
            ExprKind::Param(param) => self.params.get(*param).copied().flatten(),
            ExprKind::Local(slot) => cx.locals[*slot],
            ExprKind::Total(_, agg) | ExprKind::AccountTotal { agg, .. } => {
                Some(Kind::Total(agg.unit()))
            }
            ExprKind::Neg(x) => self.kind(x, below),
            ExprKind::Per(x, unit) => {
                let per = Cx {
                    per: Some(*unit),
                    ..below
                };
                let found = self.kind(x, per)?;
                self.per(x, found, *unit, span)
            }
            ExprKind::Limit(x, unit) => {
                let per = Cx {
                    per: Some(*unit),
                    ..below
                };
                let found = self.kind(x, per)?;
                self.limit(x, found, *unit, span)
            }
            ExprKind::Not(x) => {
                self.kind(x, below);
                Some(Kind::Number)
            }
            ExprKind::Bin(a, op, b) => {
                let (ka, kb) = (self.kind(a, below), self.kind(b, below));
                let (ka, kb) = (ka?, kb?);
                match op {
                    BinOp::And | BinOp::Or => Some(Kind::Number),
                    BinOp::Union | BinOp::Inter => {
                        unreachable!("the resolver rejects sets of days")
                    }
                    BinOp::Mul => self.multiply(ka, kb, span),
                    BinOp::Div => self.divide(ka, b, kb, span),
                    BinOp::Add | BinOp::Sub => {
                        self.combine(a, ka, b, kb, Combiner::Op(*op), span, cx)
                    }
                    BinOp::Lt
                    | BinOp::LtEq
                    | BinOp::Gt
                    | BinOp::GtEq
                    | BinOp::Eq
                    | BinOp::NotEq => {
                        self.combine(a, ka, b, kb, Combiner::Op(*op), span, cx)?;
                        Some(Kind::Number)
                    }
                }
            }
            ExprKind::If(cond, then, else_) => {
                self.kind(cond, below);
                let (kt, ke) = (self.kind(then, cx), self.kind(else_, cx));
                self.combine(then, kt?, else_, ke?, Combiner::If, span, cx)
            }
            ExprKind::Builtin(builtin, args) => match builtin {
                Builtin::Min | Builtin::Max => {
                    let [a, b] = args.as_mut_slice() else {
                        unreachable!("the resolver checks arity");
                    };
                    let (ka, kb) = (self.kind(a, cx), self.kind(b, cx));
                    let combiner = Combiner::Builtin(*builtin);
                    self.combine(a, ka?, b, kb?, combiner, span, cx)
                }
                Builtin::Abs | Builtin::Floor | Builtin::Ceil | Builtin::Round => {
                    self.kind(&mut args[0], below)
                }
                Builtin::Fill => unreachable!("handled above"),
            },
            ExprKind::Fill(..) => Some(Kind::Amount),
            ExprKind::Draw(dist, args, slot) => {
                let kinds: Vec<Option<Kind>> =
                    args.iter_mut().map(|a| self.kind(a, below)).collect();
                let kinds = kinds.into_iter().collect::<Option<Vec<_>>>()?;
                let kind = self.draw(*dist, &kinds, args, span)?;
                self.draws[*slot].period = match kind {
                    Kind::Rate(unit) | Kind::Limit(unit) => unit,
                    _ => cx.per.unwrap_or(TimeUnit::Day),
                };
                Some(kind)
            }
            ExprKind::Call(f, args) => {
                let kinds: Vec<Option<Kind>> =
                    args.iter_mut().map(|a| self.kind(a, below)).collect();
                let kinds = kinds.into_iter().collect::<Option<Vec<_>>>()?;
                self.call(*f, kinds, span)
            }
        }
    }

    /// The kind of a draw from `dist`, whose arguments `args` are of `kinds`:
    /// that of its arguments, which must agree like `min`'s and `max`'s.
    fn draw(&mut self, dist: Dist, kinds: &[Kind], args: &[Expr], span: Span) -> Option<Kind> {
        let name = dist.name();
        let kind = match (dist, kinds) {
            (Dist::Chance, [Kind::Number]) => Kind::Number,
            (Dist::Chance, [found]) => {
                self.error(Diagnostic::new(
                    args[0].span,
                    format!("`chance` takes a plain probability, like `4%`, not {found}"),
                ));
                return None;
            }
            (_, [Kind::Number, k] | [k, Kind::Number]) => *k,
            (_, [a, b]) if a == b => *a,
            (_, [a, b]) => {
                self.error(
                    Diagnostic::new(span, format!("`{name}` mixes {a} with {b}"))
                        .with_note(args[0].span, a.to_string())
                        .with_note(args[1].span, b.to_string()),
                );
                return None;
            }
            _ => unreachable!("the resolver checks arity"),
        };
        // A draw of amounts so far is an amount.
        Some(match kind {
            Kind::Total(_) => Kind::Amount,
            kind => kind,
        })
    }

    /// The kind of `x / unit`, where `x` is of kind `found`, converting `x`
    /// if it's a rate per another unit.
    fn per(&mut self, x: &mut Expr, found: Kind, unit: TimeUnit, span: Span) -> Option<Kind> {
        let message = match found {
            Kind::Number | Kind::Amount => return Some(Kind::Rate(unit)),
            // A limit becomes a rate of its amount each period.
            Kind::Rate(from) | Kind::Limit(from) => match from.conversion_to(unit) {
                // Function bodies are shared by every call, so they can't
                // convert a rate that's only per month in some.
                Some((mul, div)) if self.calls.is_empty() => {
                    scale(x, mul, div);
                    return Some(Kind::Rate(unit));
                }
                Some(_) => format!(
                    "can't convert {found} to per {unit} inside a function; \
                     convert it before passing it in"
                ),
                None => {
                    let (short, long) = (from.min(unit), from.max(unit));
                    format!(
                        "can't convert {found} to per {unit}: \
                         a {long} isn't a fixed number of {short}s"
                    )
                }
            },
            Kind::Total(_) => format!(
                "`/{unit}` can't apply to {found}, which is an amount so far, \
                 not per period"
            ),
        };
        self.error(Diagnostic::new(span, message));
        None
    }

    /// The kind of `x[unit]`, a limit, where `x` is of kind `found`,
    /// converting `x` if it's a rate.
    fn limit(&mut self, x: &mut Expr, found: Kind, unit: TimeUnit, span: Span) -> Option<Kind> {
        let message = match found {
            Kind::Number => return Some(Kind::Limit(unit)),
            Kind::Rate(from) => match from.conversion_to(unit) {
                Some((mul, div)) if self.calls.is_empty() => {
                    scale(x, mul, div);
                    return Some(Kind::Limit(unit));
                }
                Some(_) => format!(
                    "can't convert {found} to each {unit} inside a function; \
                     convert it before passing it in"
                ),
                None => {
                    let (short, long) = (from.min(unit), from.max(unit));
                    format!(
                        "can't convert {found} to each {unit}: \
                         a {long} isn't a fixed number of {short}s"
                    )
                }
            },
            Kind::Amount | Kind::Total(_) | Kind::Limit(_) => {
                format!("`[{unit}]` makes a limit of a number or a rate, not {found}")
            }
        };
        self.error(Diagnostic::new(span, message));
        None
    }

    /// Turns what a move moves into an amount: the firing's share of a rate,
    /// or what's left of a limit.
    fn settle(&mut self, e: &mut Expr, kind: Kind) -> Kind {
        match kind {
            Kind::Limit(unit) => self.make_fill(e, unit, e.span),
            kind => spread(e, kind),
        }
    }

    /// Wraps `e`, an amount each `unit` that a move moves, in a
    /// [`ExprKind::Fill`], which warnings about it point at `span`.
    fn make_fill(&mut self, e: &mut Expr, unit: TimeUnit, span: Span) -> Kind {
        let (entry, tag) = self.posting.expect("only moves fill");
        let slot = self.fills.len();
        self.fills.push(Fill {
            entry,
            tag,
            unit,
            span,
        });
        self.posting_fills.push(slot);
        wrap(e, |inner| ExprKind::Fill(inner, unit, slot));
        Kind::Amount
    }

    /// Checks `fill(target)` and turns it into an [`ExprKind::Fill`].
    fn fill(&mut self, e: &mut Expr, cx: Cx) -> Option<Kind> {
        let span = e.span;
        let ExprKind::Builtin(_, args) = &mut e.kind else {
            unreachable!("only called on `fill`");
        };
        // The target is an amount for the whole period, not what's posted, so
        // its rates aren't spread.
        let as_target = Cx {
            posting: false,
            posted: false,
            ..cx
        };
        let found = self.kind(&mut args[0], as_target)?;
        let (true, Some((_, leg))) = (cx.posted, self.posting) else {
            self.error(Diagnostic::new(
                span,
                "`fill` can only be the amount a move moves, \
                 or what `min`, `max` or `if` pick for it",
            ));
            return None;
        };
        let (Kind::Limit(unit) | Kind::Total(unit)) = found else {
            let mut diag = Diagnostic::new(
                args[0].span,
                format!(
                    "`fill` needs an amount each period, like `24_500[year]`, \
                     or a total like `x[year]`, but this is {found}"
                ),
            );
            if let Kind::Rate(unit) = found {
                diag = diag.with_note(
                    args[0].span,
                    format!("a rate is spread over its {unit}; for the whole {unit}'s amount, write `[{unit}]` after it"),
                );
            }
            self.error(diag);
            return None;
        };
        if let Some(leg) = leg
            && reads_total(&args[0], leg)
        {
            self.error(Diagnostic::new(
                args[0].span,
                "`fill` already subtracts what this move has moved, \
                 so this subtracts it twice",
            ));
            return None;
        }
        // `fill(x)` becomes `x`, filled.
        *e = args.pop().expect("the resolver checks arity");
        let kind = self.make_fill(e, unit, span);
        e.span = span;
        Some(kind)
    }

    /// The kind of combining `a` and `b` with `what`, where they must measure
    /// the same thing: the operands of `+`, `-` and comparisons, or the values
    /// that `min`, `max` and `if` pick between.
    #[allow(clippy::too_many_arguments)]
    fn combine(
        &mut self,
        a: &mut Expr,
        ka: Kind,
        b: &mut Expr,
        kb: Kind,
        what: Combiner,
        span: Span,
        cx: Cx,
    ) -> Option<Kind> {
        use Kind::*;
        let spread_both = |a: &mut Expr, b: &mut Expr| (spread(a, ka), spread(b, kb));
        let settle_limit = |checker: &mut Self, e: &mut Expr, k: Kind| match k {
            Limit(unit) => checker.make_fill(e, unit, e.span),
            k => k,
        };
        match (ka, kb) {
            (Number, k) | (k, Number) => Some(k),
            _ if ka == kb => Some(ka),
            // What `min`, `max` and `if` pick for a move to move is filled if
            // it's a limit.
            (Limit(_), _) | (_, Limit(_)) if cx.posted && what.picks() => {
                let ka = settle_limit(self, a, ka);
                let kb = settle_limit(self, b, kb);
                self.combine(a, ka, b, kb, what, span, cx)
            }
            // In an entry, `min`, `max` and `if` pick the amount to move, so
            // a rate among the choices is the firing's share of it.
            (Rate(_), _) | (_, Rate(_)) if cx.posting && what.picks() => {
                let (ka, kb) = spread_both(a, b);
                self.combine(a, ka, b, kb, what, span, cx)
            }
            // Both measure the current period: a yearly limit minus a `[year]`
            // total is what's left of the limit.
            (Limit(u), Total(v)) | (Total(v), Limit(u)) if u == v => Some(Total(u)),
            // In an entry, a rate is the firing's share of it.
            (Rate(_), _) | (_, Rate(_)) if cx.posting => {
                let (ka, kb) = spread_both(a, b);
                self.combine(a, ka, b, kb, what, span, cx)
            }
            (Amount | Total(_), Amount | Total(_)) => Some(Amount),
            _ => {
                // Function bodies don't spread rates, even in entries.
                let describe = |kind| match kind {
                    Rate(_) if self.calls.is_empty() => {
                        format!("{kind}, which only an entry can turn into an amount")
                    }
                    Limit(_) if cx.posting => format!(
                        "{kind}, which a move only fills when it's all it moves, \
                         or what `min`, `max` or `if` pick"
                    ),
                    _ => kind.to_string(),
                };
                let mut diag = Diagnostic::new(span, format!("{what} mixes {ka} with {kb}"))
                    .with_note(a.span, describe(ka))
                    .with_note(b.span, describe(kb));
                if let (Rate(u), Total(v)) | (Total(v), Rate(u)) = (ka, kb)
                    && u == v
                {
                    let rate = if matches!(ka, Rate(_)) {
                        a.span
                    } else {
                        b.span
                    };
                    diag = diag.with_note(
                        rate,
                        format!(
                            "a rate is spread over its {u}; for the whole {u}'s amount, \
                             like a limit, write `[{u}]` after it"
                        ),
                    );
                }
                self.error(diag);
                None
            }
        }
    }

    fn multiply(&mut self, ka: Kind, kb: Kind, span: Span) -> Option<Kind> {
        use Kind::*;
        match (ka, kb) {
            (Number, k) | (k, Number) => Some(k),
            (Limit(_), _) | (_, Limit(_)) => {
                self.error(Diagnostic::new(
                    span,
                    format!("can't multiply {ka} by {kb}"),
                ));
                None
            }
            (Rate(_), Rate(_)) => {
                self.error(Diagnostic::new(
                    span,
                    format!("can't multiply {ka} by {kb}"),
                ));
                None
            }
            (Rate(u), _) | (_, Rate(u)) => Some(Rate(u)),
            _ => Some(Amount),
        }
    }

    fn divide(&mut self, ka: Kind, b: &Expr, kb: Kind, span: Span) -> Option<Kind> {
        use Kind::*;
        match (ka, kb) {
            (k, Number) => Some(k),
            // Ratios, like a balance over a balance.
            (Rate(u), Rate(v)) | (Limit(u), Limit(v)) if u == v => Some(Number),
            (Amount | Total(_), Amount | Total(_)) => Some(Number),
            (Rate(u), Amount | Total(_)) => Some(Rate(u)),
            (Number, Amount | Total(_)) => Some(Amount),
            (_, Rate(unit)) => {
                let mut diag = Diagnostic::new(span, format!("can't divide {ka} by {kb}"));
                if let ExprKind::Per(..) = b.kind {
                    diag = diag.with_note(
                        b.span,
                        format!(
                            "`/{unit}` applies to what's on its left; to divide first, \
                             write `(… / …)/{unit}`"
                        ),
                    );
                }
                self.error(diag);
                None
            }
            _ => {
                self.error(Diagnostic::new(span, format!("can't divide {ka} by {kb}")));
                None
            }
        }
    }

    /// What user function `f` returns when called with arguments of `args`
    /// kinds, checking its body for them on first use.
    fn call(&mut self, f: usize, args: Vec<Kind>, span: Span) -> Option<Kind> {
        let key = (f, args);
        if let Some(kind) = self.instances.get(&key) {
            return *kind;
        }
        let mut locals = vec![None; self.fns[f].slots];
        for (local, arg) in locals.iter_mut().zip(&key.1) {
            *local = Some(*arg);
        }
        // Calls can't recurse, so this body isn't being checked further up.
        let mut body = std::mem::take(&mut self.fns[f].body);
        self.calls.push((f, span));
        let mut result = None;
        for stmt in &mut body {
            let cx = Cx {
                posting: false,
                posted: false,
                locals: &locals,
                local_draws: &[],
                per: None,
            };
            match stmt {
                Stmt::Let { slot, value, .. } => locals[*slot] = self.kind(value, cx),
                Stmt::Return(e) => {
                    result = self.kind(e, cx);
                    break;
                }
            }
        }
        self.calls.pop();
        self.fns[f].body = body;
        self.instances.insert(key, result);
        result
    }
}

/// Whether `e` reads a total of `tag`.
fn reads_total(e: &Expr, tag: usize) -> bool {
    let reads = |e: &Expr| reads_total(e, tag);
    match &e.kind {
        ExprKind::Total(total, _) => *total == tag,
        ExprKind::Num(_)
        | ExprKind::Bool(_)
        | ExprKind::Balance { .. }
        | ExprKind::AccountTotal { .. }
        | ExprKind::Param(_)
        | ExprKind::Local(_) => false,
        ExprKind::Neg(x)
        | ExprKind::Not(x)
        | ExprKind::Per(x, _)
        | ExprKind::Limit(x, _)
        | ExprKind::Spread(x, _)
        | ExprKind::Fill(x, ..) => reads(x),
        ExprKind::Bin(a, _, b) => reads(a) || reads(b),
        ExprKind::If(c, t, f) => reads(c) || reads(t) || reads(f),
        ExprKind::Builtin(_, args) | ExprKind::Call(_, args) | ExprKind::Draw(_, args, _) => {
            args.iter().any(reads)
        }
    }
}

/// Spreads `e` over the firing entry's firings if it's a rate, returning its
/// kind afterwards.
fn spread(e: &mut Expr, kind: Kind) -> Kind {
    match kind {
        Kind::Rate(unit) => {
            wrap(e, |inner| ExprKind::Spread(inner, unit));
            Kind::Amount
        }
        _ => kind,
    }
}

/// Multiplies `e` by `mul / div`.
fn scale(e: &mut Expr, mul: u32, div: u32) {
    let span = e.span;
    let num = |n: u32| {
        Box::new(Expr {
            kind: ExprKind::Num(n.into()),
            span,
        })
    };
    if mul != 1 {
        wrap(e, |inner| ExprKind::Bin(inner, BinOp::Mul, num(mul)));
    }
    if div != 1 {
        wrap(e, |inner| ExprKind::Bin(inner, BinOp::Div, num(div)));
    }
}

/// Replaces `e` with `f(e)`.
fn wrap(e: &mut Expr, f: impl FnOnce(Box<Expr>) -> ExprKind) {
    let span = e.span;
    let placeholder = Expr {
        kind: ExprKind::Bool(false),
        span,
    };
    let inner = std::mem::replace(e, placeholder);
    *e = Expr {
        kind: f(Box::new(inner)),
        span,
    };
}
