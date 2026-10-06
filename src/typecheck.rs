//! Checks that numbers and bools are used where each is expected, so that
//! mistakes are reported before the simulation runs rather than on the day an
//! expression is first evaluated.

use crate::ast::{BinOp, Dist, EntryStmt, Expr, Span, SpannedExpr, Stmt};
use crate::errors::Diagnostic;
use crate::resolver::{FnDef, Model, ParamBody};
use indexmap::IndexMap;
use std::collections::HashMap;
use std::fmt;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Ty {
    Num,
    Bool,
}

impl fmt::Display for Ty {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Ty::Num => "number",
            Ty::Bool => "bool",
        })
    }
}

/// Type-checks every expression in `model`, whose names all resolve.
pub fn check(model: &Model) -> Vec<Diagnostic> {
    let mut checker = Checker {
        fns: &model.fns,
        params: &model.params,
        returns: HashMap::new(),
        param_types: HashMap::new(),
        locals: HashMap::new(),
        diags: Vec::new(),
    };
    for name in model.fns.keys() {
        checker.fn_return(name);
    }
    for account in model.accounts.values() {
        if let Some((e, _)) = &account.opening {
            checker.expect(e, Ty::Num, "an opening balance");
        }
    }
    for name in model.params.keys() {
        checker.param_type(name);
    }
    for entry in &model.entries {
        checker.locals.clear();
        EntryStmt::visit(&entry.body, &mut |stmt| match stmt {
            EntryStmt::Let { name, value } => {
                let ty = checker.type_of(value);
                checker.set_local(&name.0, ty);
            }
            EntryStmt::Move(m) => checker.expect(&m.amount, Ty::Num, "an amount"),
            EntryStmt::If { branches, .. } => {
                for (cond, _) in branches {
                    checker.expect(cond, Ty::Bool, "an `if` condition");
                }
            }
            EntryStmt::For { .. } => unreachable!("the resolver expands loops"),
        });
    }
    checker.locals.clear();
    for so_far in &model.totals_so_far {
        checker.expect(&so_far.value, Ty::Num, "a total");
    }
    for (_, e) in &model.checks {
        checker.expect(e, Ty::Bool, "a check");
    }
    checker.diags.sort_by_key(|d| d.span.start);
    checker.diags
}

struct Checker<'a> {
    fns: &'a IndexMap<String, FnDef>,
    params: &'a IndexMap<String, ParamBody>,
    /// Return types of user functions; `None` if the body has a type error
    /// (already reported) or is still being checked.
    returns: HashMap<&'a str, Option<Ty>>,
    /// Types of values, likewise.
    param_types: HashMap<&'a str, Option<Ty>>,
    /// Types of the parameters and `let`s in scope.
    locals: HashMap<String, Ty>,
    diags: Vec<Diagnostic>,
}

impl<'a> Checker<'a> {
    fn expect(&mut self, e: &SpannedExpr, expected: Ty, what: &str) {
        if let Some(found) = self.type_of(e)
            && found != expected
        {
            self.diags.push(Diagnostic::new(
                e.1,
                format!("{what} must be a {expected}, but this is a {found}"),
            ));
        }
    }

    /// Gives local `name` type `ty`, or forgets it if `ty` is `None`, so that
    /// its uses aren't reported again.
    fn set_local(&mut self, name: &str, ty: Option<Ty>) {
        match ty {
            Some(ty) => self.locals.insert(name.to_string(), ty),
            None => self.locals.remove(name),
        };
    }

    /// The return type of user function `name`, checking its body on first use.
    fn fn_return(&mut self, name: &str) -> Option<Ty> {
        let (name, def) = self.fns.get_key_value(name)?;
        if let Some(ty) = self.returns.get(name.as_str()) {
            return *ty;
        }
        // Guards against recursion, which the resolver reports separately.
        self.returns.insert(name, None);
        let outer = std::mem::take(&mut self.locals);
        for param in &def.params {
            self.locals.insert(param.clone(), Ty::Num);
        }
        let mut ty = None;
        for stmt in &def.body {
            match stmt {
                Stmt::Let { name, value } => {
                    let ty = self.type_of(value);
                    self.set_local(name, ty);
                }
                Stmt::Return(e) => ty = self.type_of(e),
                Stmt::Mod(_) => unreachable!("functions that make modules aren't checked"),
            }
        }
        self.locals = outer;
        self.returns.insert(name, ty);
        ty
    }

    /// The type of value `name`, checking its body on first use: a number or
    /// a bool, which all its arms must agree on.
    fn param_type(&mut self, name: &str) -> Option<Ty> {
        let (name, body) = self.params.get_key_value(name)?;
        if let Some(ty) = self.param_types.get(name.as_str()) {
            return *ty;
        }
        // Guards against cycles, which the resolver reports separately.
        self.param_types.insert(name, None);
        let outer = std::mem::take(&mut self.locals);
        let ty = match body {
            ParamBody::Const(e) => self.type_of(e),
            ParamBody::Arms(arms) => {
                let mut agreed: Option<(Ty, Span)> = None;
                let mut failed = false;
                for arm in arms {
                    if let Some(growth) = &arm.growth {
                        self.expect(&arm.value, Ty::Num, "a value that grows");
                        self.expect(&growth.by, Ty::Num, "a growth rate");
                    }
                    let Some(ty) = self.type_of(&arm.value) else {
                        failed = true;
                        continue;
                    };
                    match agreed {
                        None => agreed = Some((ty, arm.value.1)),
                        Some((first, first_span)) if first != ty => {
                            self.diags.push(
                                Diagnostic::new(
                                    arm.value.1,
                                    format!(
                                        "`{name}` is a {first} in one arm but a {ty} in another"
                                    ),
                                )
                                .with_note(first_span, format!("a {first}")),
                            );
                            failed = true;
                        }
                        Some(_) => {}
                    }
                }
                if failed {
                    None
                } else {
                    agreed.map(|(ty, _)| ty)
                }
            }
        };
        self.locals = outer;
        self.param_types.insert(name, ty);
        ty
    }

    /// The type of `e`, or `None` if it contains an error (already reported).
    fn type_of(&mut self, (expr, span): &SpannedExpr) -> Option<Ty> {
        match expr.as_ref() {
            Expr::Global(name) => self.param_type(name),
            Expr::Num(_) | Expr::Balance(_) | Expr::TagTotal { .. } | Expr::AccountTotal(..) => {
                Some(Ty::Num)
            }
            Expr::Local(name) => self.locals.get(name).copied(),
            Expr::Bool(_) => Some(Ty::Bool),
            Expr::Neg(x) => {
                self.expect(x, Ty::Num, "the operand of `-`");
                Some(Ty::Num)
            }
            Expr::Per(x, unit) => {
                self.expect(x, Ty::Num, &format!("the operand of `/{unit}`"));
                Some(Ty::Num)
            }
            Expr::Period(x, unit) => {
                self.expect(x, Ty::Num, &format!("the operand of `[{unit}]`"));
                Some(Ty::Num)
            }
            Expr::Not(x) => {
                self.expect(x, Ty::Bool, "the operand of `not`");
                Some(Ty::Bool)
            }
            Expr::Bin(a, op, b) => match op {
                BinOp::Add | BinOp::Sub | BinOp::Mul | BinOp::Div | BinOp::Pow => {
                    self.operands(a, b, *op, Ty::Num);
                    Some(Ty::Num)
                }
                BinOp::Lt | BinOp::LtEq | BinOp::Gt | BinOp::GtEq => {
                    self.operands(a, b, *op, Ty::Num);
                    Some(Ty::Bool)
                }
                BinOp::And | BinOp::Or => {
                    self.operands(a, b, *op, Ty::Bool);
                    Some(Ty::Bool)
                }
                BinOp::Eq | BinOp::NotEq => {
                    let (x, y) = (self.type_of(a)?, self.type_of(b)?);
                    if x != y {
                        self.diags.push(Diagnostic::new(
                            *span,
                            format!("`{op}` compares a {x} with a {y}"),
                        ));
                    }
                    Some(Ty::Bool)
                }
                BinOp::Union | BinOp::Inter => unreachable!("the resolver rejects sets of days"),
            },
            Expr::If { cond, then, else_ } => {
                self.expect(cond, Ty::Bool, "an `if` condition");
                let (x, y) = (self.type_of(then)?, self.type_of(else_)?);
                if x != y {
                    self.diags.push(Diagnostic::new(
                        *span,
                        format!("`if` branches must have the same type, but are a {x} and a {y}"),
                    ));
                    return None;
                }
                Some(x)
            }
            Expr::Call(callee, args) => {
                for arg in args {
                    self.expect(arg, Ty::Num, &format!("an argument to `{callee}`"));
                }
                let name = callee.join();
                if self.fns.contains_key(&name) {
                    self.fn_return(&name)
                } else {
                    Some(Ty::Num) // builtins all return numbers
                }
            }
            Expr::Draw(dist, args, _) => {
                for arg in args {
                    self.expect(arg, Ty::Num, &format!("an argument to `{}`", dist.name()));
                }
                Some(match dist {
                    Dist::Chance => Ty::Bool,
                    Dist::Normal | Dist::Lognormal | Dist::Uniform => Ty::Num,
                })
            }
            Expr::Ref(_) | Expr::Days(_) | Expr::Range { .. } | Expr::Named(..) => {
                unreachable!("the resolver replaces these")
            }
        }
    }

    fn operands(&mut self, a: &SpannedExpr, b: &SpannedExpr, op: BinOp, expected: Ty) {
        let what = format!("an operand of `{op}`");
        self.expect(a, expected, &what);
        self.expect(b, expected, &what);
    }
}
