//! Checks that numbers and bools are used where each is expected, so that
//! mistakes are reported before the simulation runs rather than on the day an
//! expression is first evaluated.

use crate::ast::{BinOp, Decl, Expr, ParamBody, PostingAmount, Program, SpannedExpr, Stmt};
use crate::errors::Diagnostic;
use crate::resolver::FnDef;
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

/// Type-checks every expression in `program`. Expects names and call arities
/// to have been validated already.
pub fn check(program: &Program, fns: &HashMap<String, FnDef>) -> Vec<Diagnostic> {
    let mut checker = Checker {
        fns,
        returns: HashMap::new(),
        diags: Vec::new(),
    };

    let mut fn_names: Vec<&str> = fns.keys().map(String::as_str).collect();
    fn_names.sort();
    for name in fn_names {
        checker.fn_return(name);
    }

    for (decl, _) in &program.decls {
        match decl {
            Decl::Account {
                opening: Some((e, _)),
                ..
            } => checker.expect(e, Ty::Num, "an opening balance"),
            Decl::Param { body, .. } => match body {
                ParamBody::Const(e) => checker.expect(e, Ty::Num, "a param"),
                ParamBody::Schedule(intervals) => {
                    for iv in intervals {
                        checker.expect(&iv.value, Ty::Num, "a param");
                    }
                }
            },
            Decl::Entry { postings, .. } => {
                for posting in postings {
                    if let Some(PostingAmount::Expr(e)) = &posting.amount {
                        checker.expect(e, Ty::Num, "a posting amount");
                    }
                }
            }
            Decl::Assert { asserted, .. } => checker.expect(asserted, Ty::Bool, "an assertion"),
            Decl::Account { .. }
            | Decl::Schedule { .. }
            | Decl::Fn { .. }
            | Decl::Import { .. } => {}
        }
    }
    checker.diags
}

struct Checker<'a> {
    fns: &'a HashMap<String, FnDef>,
    /// Return types of user functions; `None` if the body has a type error
    /// (already reported) or is still being checked.
    returns: HashMap<&'a str, Option<Ty>>,
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
        let mut ty = None;
        for stmt in &def.body {
            match stmt {
                Stmt::Let { value, .. } => self.expect(value, Ty::Num, "a `let` binding"),
                Stmt::Return(e) => ty = self.type_of(e),
            }
        }
        self.returns.insert(name, ty);
        ty
    }

    /// The type of `e`, or `None` if it contains an error (already reported).
    fn type_of(&mut self, (expr, span): &SpannedExpr) -> Option<Ty> {
        match expr.as_ref() {
            Expr::Num(_) | Expr::Ref(_) | Expr::ParamAgg(..) => Some(Ty::Num),
            Expr::Bool(_) => Some(Ty::Bool),
            Expr::Neg(x) => {
                self.expect(x, Ty::Num, "the operand of `-`");
                Some(Ty::Num)
            }
            Expr::Per(x, unit) => {
                self.expect(x, Ty::Num, &format!("the operand of `per {unit}`"));
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
            Expr::Call(name, args) => {
                for arg in args {
                    self.expect(arg, Ty::Num, &format!("an argument to `{name}`"));
                }
                if self.fns.contains_key(name) {
                    self.fn_return(name)
                } else {
                    Some(Ty::Num) // builtins all return numbers
                }
            }
        }
    }

    fn operands(&mut self, a: &SpannedExpr, b: &SpannedExpr, op: BinOp, expected: Ty) {
        let what = format!("an operand of `{op}`");
        self.expect(a, expected, &what);
        self.expect(b, expected, &what);
    }
}
