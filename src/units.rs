//! Checks how rates and amounts combine, and turns rates into amounts where
//! entries post them.
//!
//! `per year` and the like make a value a rate. A posting moves an amount,
//! not a rate, so wherever a posting uses a rate as an amount, this pass wraps
//! it in [`ExprKind::Spread`] to give each firing its share (see [`Schedule::share`](crate::ast::Schedule::share)). Anywhere else,
//! mixing a rate with an amount is an error, except with a total over the same
//! period: a per-year limit minus a `.ytd` total is what's left of the limit.

use crate::ast::{BinOp, Span, TimeUnit};
use crate::compile::{Amount, Builtin, Expr, ExprKind, Function, Param, ParamBody, Program, Stmt};
use crate::errors::Diagnostic;
use std::collections::HashMap;
use std::fmt;

/// What a number measures, as far as time goes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum Kind {
    /// A plain number, like a literal, which takes on the kind of whatever
    /// it's combined with.
    Number,
    /// An amount, like a balance or a posting.
    Amount,
    /// A `.ytd`, `.qtd` or `.mtd` total: the amount so far this period.
    Total(TimeUnit),
    /// An amount per period, like a salary `per year`.
    Rate(TimeUnit),
}

impl fmt::Display for Kind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Kind::Number => f.write_str("a number"),
            Kind::Amount => f.write_str("an amount"),
            Kind::Total(unit) => write!(f, "a {unit}-to-date total"),
            Kind::Rate(unit) => write!(f, "an amount per {unit}"),
        }
    }
}

/// Where an expression appears.
#[derive(Clone, Copy)]
struct Cx<'a> {
    /// In a posting, where rates are spread over the entry's firings.
    posting: bool,
    /// Kinds of the current function's parameters and `let` bindings.
    locals: &'a [Option<Kind>],
}

const GLOBAL: Cx = Cx {
    posting: false,
    locals: &[],
};

/// Checks every expression in `program`, spreading the rates that postings use.
pub fn check(program: &mut Program) -> Vec<Diagnostic> {
    let mut checker = Checker {
        fns: &mut program.fns,
        params: Vec::with_capacity(program.params.len()),
        instances: HashMap::new(),
        calls: Vec::new(),
        diags: Vec::new(),
    };
    // Each param comes after the params it reads.
    for param in &mut program.params {
        let kind = checker.param(param);
        checker.params.push(kind);
    }
    for (expr, _) in program.openings.iter_mut().flatten() {
        if let Some(kind @ Kind::Rate(_)) = checker.kind(expr, GLOBAL) {
            checker.error(Diagnostic::new(
                expr.span,
                format!("an opening balance must be an amount, but this is {kind}"),
            ));
        }
    }
    let in_posting = Cx {
        posting: true,
        locals: &[],
    };
    for entry in &mut program.entries {
        for posting in &mut entry.postings {
            if let Amount::Expr(e) = &mut posting.amount
                && let Some(kind) = checker.kind(e, in_posting)
            {
                spread(e, kind);
            }
        }
    }
    for (_, e) in &mut program.asserts {
        checker.kind(e, GLOBAL);
    }
    checker.diags.sort_by_key(|d| d.span.start);
    checker.diags
}

struct Checker<'p> {
    fns: &'p mut [Function],
    /// Kinds of the params checked so far; `None` for a param with an error
    /// (already reported).
    params: Vec<Option<Kind>>,
    /// What each function returns, by function and argument kinds.
    instances: HashMap<(usize, Vec<Kind>), Option<Kind>>,
    /// The calls whose function bodies are being checked, innermost last.
    calls: Vec<(usize, Span)>,
    diags: Vec<Diagnostic>,
}

impl Checker<'_> {
    /// Reports `diag`, pointing at the calls that led to it.
    fn error(&mut self, mut diag: Diagnostic) {
        for &(f, span) in self.calls.iter().rev() {
            diag = diag.with_note(span, format!("in this call to `{}`", self.fns[f].name));
        }
        self.diags.push(diag);
    }

    fn param(&mut self, param: &mut Param) -> Option<Kind> {
        let name = &param.name;
        match &mut param.body {
            ParamBody::Const(e) => self.kind(e, GLOBAL),
            ParamBody::Intervals(intervals) => {
                let found: Vec<(Option<Kind>, Span)> = intervals
                    .iter_mut()
                    .map(|iv| (self.kind(&mut iv.value, GLOBAL), iv.value.span))
                    .collect();
                // The param is whatever its intervals agree on.
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
                            let (first, first_span) = first.expect("an earlier interval set it");
                            self.error(
                                Diagnostic::new(
                                    span,
                                    format!(
                                        "param `{name}` is {first} in one interval \
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

    /// The kind of `e`, or `None` if it has an error (already reported).
    fn kind(&mut self, e: &mut Expr, cx: Cx) -> Option<Kind> {
        let span = e.span;
        match &mut e.kind {
            ExprKind::Num(_) | ExprKind::Bool(_) => Some(Kind::Number),
            ExprKind::Account(_) | ExprKind::Leg(_) | ExprKind::Spread(..) => Some(Kind::Amount),
            ExprKind::Param(param) => self.params.get(*param).copied().flatten(),
            ExprKind::Local(slot) => cx.locals[*slot],
            ExprKind::Total(_, agg) => Some(Kind::Total(agg.unit())),
            ExprKind::Neg(x) => self.kind(x, cx),
            ExprKind::Per(x, unit) => {
                let unit = *unit;
                let found = self.kind(x, cx)?;
                let message = match found {
                    Kind::Number | Kind::Amount => return Some(Kind::Rate(unit)),
                    Kind::Rate(from) => match from.conversion_to(unit) {
                        // Function bodies are shared by every call, so they
                        // can't convert a rate that's only per month in some.
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
                        "`per {unit}` can't apply to {found}, which is an amount so far, \
                         not per period"
                    ),
                };
                self.error(Diagnostic::new(span, message));
                None
            }
            ExprKind::Not(x) => {
                self.kind(x, cx);
                Some(Kind::Number)
            }
            ExprKind::Bin(a, op, b) => {
                let (ka, kb) = (self.kind(a, cx), self.kind(b, cx));
                let (ka, kb) = (ka?, kb?);
                match op {
                    BinOp::And | BinOp::Or => Some(Kind::Number),
                    BinOp::Mul => self.multiply(ka, kb, span),
                    BinOp::Div => self.divide(ka, b, kb, span),
                    BinOp::Add | BinOp::Sub => {
                        self.combine(a, ka, b, kb, &format!("`{op}`"), false, span, cx)
                    }
                    BinOp::Lt
                    | BinOp::LtEq
                    | BinOp::Gt
                    | BinOp::GtEq
                    | BinOp::Eq
                    | BinOp::NotEq => {
                        self.combine(a, ka, b, kb, &format!("`{op}`"), false, span, cx)?;
                        Some(Kind::Number)
                    }
                }
            }
            ExprKind::If(cond, then, else_) => {
                self.kind(cond, cx);
                let (kt, ke) = (self.kind(then, cx), self.kind(else_, cx));
                self.combine(then, kt?, else_, ke?, "this `if`", true, span, cx)
            }
            ExprKind::Builtin(builtin, args) => match builtin {
                Builtin::Min | Builtin::Max => {
                    let [a, b] = args.as_mut_slice() else {
                        unreachable!("the resolver checks arity");
                    };
                    let (ka, kb) = (self.kind(a, cx), self.kind(b, cx));
                    let what = format!("`{}`", builtin.name());
                    self.combine(a, ka?, b, kb?, &what, true, span, cx)
                }
                Builtin::Abs | Builtin::Floor | Builtin::Ceil | Builtin::Round => {
                    self.kind(&mut args[0], cx)
                }
            },
            ExprKind::Call(f, args) => {
                let kinds: Vec<Option<Kind>> = args.iter_mut().map(|a| self.kind(a, cx)).collect();
                let kinds = kinds.into_iter().collect::<Option<Vec<_>>>()?;
                self.call(*f, kinds, span)
            }
        }
    }

    /// The kind of combining `a` and `b`, which must measure the same thing:
    /// the operands of `+`, `-` and comparisons, or the values that `min`,
    /// `max` and `if` pick between (`picks`). `what` names the operation.
    #[allow(clippy::too_many_arguments)]
    fn combine(
        &mut self,
        a: &mut Expr,
        ka: Kind,
        b: &mut Expr,
        kb: Kind,
        what: &str,
        picks: bool,
        span: Span,
        cx: Cx,
    ) -> Option<Kind> {
        use Kind::*;
        let spread_both = |a: &mut Expr, b: &mut Expr| (spread(a, ka), spread(b, kb));
        match (ka, kb) {
            (Number, k) | (k, Number) => Some(k),
            _ if ka == kb => Some(ka),
            // In a posting, `min`, `max` and `if` pick the amount to post, so
            // a rate among the choices is the firing's share of it.
            (Rate(_), _) | (_, Rate(_)) if cx.posting && picks => {
                let (ka, kb) = spread_both(a, b);
                self.combine(a, ka, b, kb, what, picks, span, cx)
            }
            // Both measure the current period, so the rate counts in full: a
            // per-year limit minus a `.ytd` total is what's left of the limit.
            (Rate(u), Total(v)) | (Total(v), Rate(u)) if u == v => Some(Total(u)),
            // In a posting, a rate is the firing's share of it.
            (Rate(_), _) | (_, Rate(_)) if cx.posting => {
                let (ka, kb) = spread_both(a, b);
                self.combine(a, ka, b, kb, what, picks, span, cx)
            }
            (Amount | Total(_), Amount | Total(_)) => Some(Amount),
            _ => {
                // Function bodies don't spread rates, even in postings.
                let describe = |kind| match kind {
                    Rate(_) if self.calls.is_empty() => {
                        format!("{kind}, which only a posting can turn into an amount")
                    }
                    _ => kind.to_string(),
                };
                let diag = Diagnostic::new(span, format!("{what} mixes {ka} with {kb}"))
                    .with_note(a.span, describe(ka))
                    .with_note(b.span, describe(kb));
                self.error(diag);
                None
            }
        }
    }

    fn multiply(&mut self, ka: Kind, kb: Kind, span: Span) -> Option<Kind> {
        use Kind::*;
        match (ka, kb) {
            (Number, k) | (k, Number) => Some(k),
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
            (Rate(u), Rate(v)) if u == v => Some(Number),
            (Amount | Total(_), Amount | Total(_)) => Some(Number),
            (Rate(u), Amount | Total(_)) => Some(Rate(u)),
            (Number, Amount | Total(_)) => Some(Amount),
            (_, Rate(unit)) => {
                let mut diag = Diagnostic::new(span, format!("can't divide {ka} by {kb}"));
                if let ExprKind::Per(..) = b.kind {
                    diag = diag.with_note(
                        b.span,
                        format!(
                            "`per` binds tighter than `/`; to divide first, \
                             write `(… / …) per {unit}`"
                        ),
                    );
                }
                self.error(diag);
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
                locals: &locals,
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
