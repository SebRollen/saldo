//! Checks that numbers and bools are used where each is expected, so that
//! mistakes are reported before the simulation runs rather than on the day an
//! expression is first evaluated.

use crate::ast::{BinOp, EntryStmt, Expr, SpannedExpr, Stmt};
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
        returns: HashMap::new(),
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
    for body in model.params.values() {
        match body {
            ParamBody::Const(e) => checker.expect(e, Ty::Num, "a value"),
            ParamBody::Arms(arms) => {
                for arm in arms {
                    checker.expect(&arm.value, Ty::Num, "a value");
                    if let Some(growth) = &arm.growth {
                        checker.expect(&growth.by, Ty::Num, "a growth rate");
                    }
                }
            }
        }
    }
    for entry in &model.entries {
        checker.locals.clear();
        EntryStmt::visit(&entry.body, &mut |stmt| match stmt {
            EntryStmt::Let { name, value } => {
                checker.expect(value, Ty::Num, "a `let`");
                checker.locals.insert(name.0.clone(), Ty::Num);
            }
            EntryStmt::Move(m) => checker.expect(&m.amount, Ty::Num, "an amount"),
            EntryStmt::If { branches, .. } => {
                for (cond, _) in branches {
                    checker.expect(cond, Ty::Bool, "an `if` condition");
                }
            }
        });
    }
    checker.locals.clear();
    for (_, e) in &model.checks {
        checker.expect(e, Ty::Bool, "a check");
    }
    checker.diags.sort_by_key(|d| d.span.start);
    checker.diags
}

struct Checker<'a> {
    fns: &'a IndexMap<String, FnDef>,
    /// Return types of user functions; `None` if the body has a type error
    /// (already reported) or is still being checked.
    returns: HashMap<&'a str, Option<Ty>>,
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
                    self.expect(value, Ty::Num, "a `let`");
                    self.locals.insert(name.clone(), Ty::Num);
                }
                Stmt::Return(e) => ty = self.type_of(e),
            }
        }
        self.locals = outer;
        self.returns.insert(name, ty);
        ty
    }

    /// The type of `e`, or `None` if it contains an error (already reported).
    fn type_of(&mut self, (expr, span): &SpannedExpr) -> Option<Ty> {
        match expr.as_ref() {
            Expr::Num(_)
            | Expr::Global(_)
            | Expr::Balance(_)
            | Expr::TagTotal { .. }
            | Expr::AccountTotal(..) => Some(Ty::Num),
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
                BinOp::Add | BinOp::Sub | BinOp::Mul | BinOp::Div => {
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
            Expr::Ref(_) | Expr::Days(_) | Expr::Range { .. } => {
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
