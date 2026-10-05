//! Expands `mod x = f(…)` into the module that function `f` returns, with its
//! parameters replaced by the arguments, before names are resolved.
//!
//! Arguments are spelled out from the top of the model first, so they mean in
//! the new module what they meant where the call is.

use crate::ast::{
    AccountKind, Decl, EntryStmt, Expr, LetBody, ModBody, Path, Program, Span, Spanned,
    SpannedExpr, Stmt,
};
use crate::errors::Diagnostic;
use std::collections::HashMap;

pub fn expand(program: &Program) -> Result<Program, Vec<Diagnostic>> {
    let mut expander = Expander::default();
    expander.index(&[], &program.decls);
    let decls = expander.decls(&[], program.decls.clone(), &[]);
    if expander.diags.is_empty() {
        Ok(Program { decls })
    } else {
        Err(expander.diags)
    }
}

/// A function that returns a module.
#[derive(Clone)]
struct ModuleFn {
    name: String,
    params: Vec<Spanned<String>>,
    /// Its `let`s, before the module.
    lets: Vec<(String, SpannedExpr)>,
    body: Vec<Spanned<Decl>>,
}

/// What a name declared in a module is, as far as expanding goes.
#[derive(Clone)]
enum Name {
    Module(Vec<String>),
    /// A function that returns a module, by its key in `Expander::fns`.
    ModuleFn(String),
    Fn,
    Other,
}

#[derive(Default)]
struct Expander {
    names: HashMap<Vec<String>, HashMap<String, Name>>,
    fns: HashMap<String, ModuleFn>,
    diags: Vec<Diagnostic>,
}

/// The names `bindings` gives values to, and what they stand for.
type Bindings = HashMap<String, SpannedExpr>;

fn child(module: &[String], name: &str) -> Vec<String> {
    module.iter().cloned().chain([name.to_string()]).collect()
}

impl Expander {
    /// Records the names declared in `decls`, the declarations of `module`.
    fn index(&mut self, module: &[String], decls: &[Spanned<Decl>]) {
        for (decl, _) in decls {
            let (name, kind) = match decl {
                Decl::Let { name, .. } | Decl::Entry { name, .. } => (name, Name::Other),
                Decl::Fn { name, params, body } => match body.split_last() {
                    Some((Stmt::Mod(decls), lets)) => {
                        let key = child(module, &name.0).join(".");
                        let lets = lets
                            .iter()
                            .filter_map(|stmt| match stmt {
                                Stmt::Let { name, value } => Some((name.clone(), value.clone())),
                                _ => None,
                            })
                            .collect();
                        self.fns.insert(
                            key.clone(),
                            ModuleFn {
                                name: name.0.clone(),
                                params: params.clone(),
                                lets,
                                body: decls.clone(),
                            },
                        );
                        (name, Name::ModuleFn(key))
                    }
                    _ => (name, Name::Fn),
                },
                Decl::Mod { name, body } => {
                    let path = child(module, &name.0);
                    if let ModBody::Inline(decls) = body {
                        self.index(&path, decls);
                    }
                    (name, Name::Module(path))
                }
                Decl::Check { .. } | Decl::Balances { .. } => continue,
            };
            self.names
                .entry(module.to_vec())
                .or_default()
                .insert(name.0.clone(), kind);
        }
    }

    /// What `path` names in `module`: a name in the module, or else at the
    /// top of the model, and then names inside that.
    fn lookup(&self, module: &[String], path: &Path) -> Option<Name> {
        let root = Vec::new();
        let (first, rest) = match path.0.split_first()? {
            (first, rest) if first == Path::ROOT => (rest.first()?, &rest[1..]),
            (first, rest) => (first, rest),
        };
        let mut name = if path.0[0] == Path::ROOT {
            self.names.get(&root)?.get(first)?.clone()
        } else {
            self.names
                .get(module)
                .and_then(|ns| ns.get(first))
                .or_else(|| self.names.get(&root)?.get(first))?
                .clone()
        };
        for part in rest {
            let Name::Module(m) = name else {
                return None;
            };
            name = self.names.get(&m)?.get(part)?.clone();
        }
        Some(name)
    }

    /// Spells out the names in `expr` that `module`, or the top of the model,
    /// declares from the top, so they mean the same in another module.
    fn qualify(&self, module: &[String], (expr, _): &mut SpannedExpr) {
        let spell_out = |path: &mut Path| {
            let first = &path.0[0];
            if first == Path::ROOT || AccountKind::from_root(first).is_some() {
                return;
            }
            let prefix: Vec<String> = if self
                .names
                .get(module)
                .is_some_and(|ns| ns.contains_key(first))
            {
                module.to_vec()
            } else if self
                .names
                .get(&Vec::new())
                .is_some_and(|ns| ns.contains_key(first))
            {
                Vec::new()
            } else {
                return;
            };
            let mut full = vec![Path::ROOT.to_string()];
            full.extend(prefix);
            full.append(&mut path.0);
            path.0 = full;
        };
        match expr.as_mut() {
            Expr::Ref(path) => spell_out(path),
            Expr::Call(callee, args) => {
                spell_out(callee);
                for arg in args {
                    self.qualify(module, arg);
                }
            }
            _ => for_each_child(expr, &mut |e| self.qualify(module, e)),
        }
    }

    /// `decls`, the declarations of `module`, with each `mod x = f(…)`
    /// expanded. `stack` holds the functions being expanded, to catch one
    /// that uses itself.
    fn decls(
        &mut self,
        module: &[String],
        decls: Vec<Spanned<Decl>>,
        stack: &[String],
    ) -> Vec<Spanned<Decl>> {
        decls
            .into_iter()
            .map(|(decl, span)| {
                let Decl::Mod { name, body } = decl else {
                    return (decl, span);
                };
                let path = child(module, &name.0);
                let decls = match body {
                    ModBody::Inline(decls) => self.decls(&path, decls, stack),
                    ModBody::Call(call) => self.instance(module, &path, &call, stack),
                    ModBody::File => Vec::new(),
                };
                let body = ModBody::Inline(decls);
                (Decl::Mod { name, body }, span)
            })
            .collect()
    }

    /// The declarations of module `path`, which `call`, in `module`, makes.
    fn instance(
        &mut self,
        module: &[String],
        path: &[String],
        call: &SpannedExpr,
        stack: &[String],
    ) -> Vec<Spanned<Decl>> {
        let Expr::Call(callee, args) = call.0.as_ref() else {
            unreachable!("the parser checks it's a call");
        };
        let key = match self.lookup(module, callee) {
            Some(Name::ModuleFn(key)) => key,
            Some(Name::Fn) => {
                return self.error(
                    call.1,
                    format!(
                        "`{callee}` returns a value, not a module; a function that makes \
                         a module ends with `mod {{ … }}`"
                    ),
                );
            }
            Some(_) => return self.error(call.1, format!("`{callee}` isn't a function")),
            None => return self.error(call.1, format!("unknown function `{callee}`")),
        };
        if stack.contains(&key) {
            return self.error(
                call.1,
                format!("`{callee}` makes a module that uses `{callee}` itself"),
            );
        }
        let f = self.fns[&key].clone();
        let Some(args) = self.bind(&f, args, call.1) else {
            return Vec::new();
        };
        let mut bindings = Bindings::new();
        for ((param, _), mut arg) in f.params.iter().zip(args) {
            self.qualify(module, &mut arg);
            bindings.insert(param.clone(), arg);
        }
        for (name, value) in &f.lets {
            let mut value = value.clone();
            substitute(&mut value, &bindings, &mut self.diags);
            bindings.insert(name.clone(), value);
        }
        let mut body = f.body.clone();
        self.check_no_shadowing(&f, &body, &bindings);
        substitute_decls(&mut body, &bindings, &mut self.diags);
        self.index(path, &body);
        let mut stack = stack.to_vec();
        stack.push(key);
        self.decls(path, body, &stack)
    }

    /// The arguments of a call to `f`, in the order of its parameters.
    fn bind(&mut self, f: &ModuleFn, args: &[SpannedExpr], span: Span) -> Option<Vec<SpannedExpr>> {
        let errors = self.diags.len();
        let args = order_args(&f.name, &f.params, args, span, &mut self.diags)?;
        (self.diags.len() == errors).then_some(args)
    }

    /// Reports names the module of `f` declares that are also its parameters
    /// or `let`s, which would be ambiguous.
    fn check_no_shadowing(&mut self, f: &ModuleFn, body: &[Spanned<Decl>], bindings: &Bindings) {
        let mut names: Vec<&Spanned<String>> = Vec::new();
        collect_names(body, &mut names);
        for (name, span) in names {
            if bindings.contains_key(name) {
                self.diags.push(Diagnostic::new(
                    *span,
                    format!(
                        "`{name}` is a parameter or `let` of `{}`, so its module can't \
                         declare `{name}` too",
                        f.name
                    ),
                ));
            }
        }
    }

    fn error(&mut self, span: Span, message: String) -> Vec<Spanned<Decl>> {
        self.diags.push(Diagnostic::new(span, message));
        Vec::new()
    }
}

/// The arguments of a call to `name`, whose parameters are `params`, in the
/// order of its parameters: those given by position, and then those given by
/// name. `None` if one is missing.
pub fn order_args(
    name: &str,
    params: &[Spanned<String>],
    args: &[SpannedExpr],
    span: Span,
    diags: &mut Vec<Diagnostic>,
) -> Option<Vec<SpannedExpr>> {
    let mut ordered: Vec<Option<SpannedExpr>> = vec![None; params.len()];
    let mut by_name = false;
    for (i, arg) in args.iter().enumerate() {
        match arg.0.as_ref() {
            Expr::Named((given, given_span), value) => {
                by_name = true;
                match params.iter().position(|(p, _)| p == given) {
                    Some(i) if ordered[i].is_some() => diags.push(Diagnostic::new(
                        *given_span,
                        format!("`{given}` is given twice"),
                    )),
                    Some(i) => ordered[i] = Some(value.clone()),
                    None => diags.push(Diagnostic::new(
                        *given_span,
                        format!("`{name}` has no parameter `{given}`"),
                    )),
                }
            }
            _ if by_name => diags.push(Diagnostic::new(
                arg.1,
                "arguments given by position go before those given by name",
            )),
            _ if i >= params.len() => {
                let word = if params.len() == 1 {
                    "argument"
                } else {
                    "arguments"
                };
                diags.push(Diagnostic::new(
                    span,
                    format!("`{name}` takes {} {word}, got {}", params.len(), args.len()),
                ));
                return None;
            }
            _ => ordered[i] = Some(arg.clone()),
        }
    }
    let missing: Vec<&str> = params
        .iter()
        .zip(&ordered)
        .filter(|(_, arg)| arg.is_none())
        .map(|((p, _), _)| p.as_str())
        .collect();
    if !missing.is_empty() {
        diags.push(Diagnostic::new(
            span,
            format!("`{name}` needs `{}`", missing.join("`, `")),
        ));
        return None;
    }
    ordered.into_iter().collect()
}

/// Calls `f` on each expression directly inside `expr`.
fn for_each_child(expr: &mut Expr, f: &mut impl FnMut(&mut SpannedExpr)) {
    match expr {
        Expr::Neg(x) | Expr::Not(x) | Expr::Per(x, _) | Expr::Period(x, _) | Expr::Named(_, x) => {
            f(x)
        }
        Expr::Bin(a, _, b) => {
            f(a);
            f(b);
        }
        Expr::If { cond, then, else_ } => {
            f(cond);
            f(then);
            f(else_);
        }
        Expr::Call(_, args) => args.iter_mut().for_each(f),
        Expr::Range { from, to, .. } => {
            f(from);
            if let Some(to) = to {
                f(to);
            }
        }
        Expr::Num(_)
        | Expr::Bool(_)
        | Expr::Ref(_)
        | Expr::Days(_)
        | Expr::Global(_)
        | Expr::Local(_)
        | Expr::Balance(_)
        | Expr::TagTotal { .. }
        | Expr::AccountTotal(..) => {}
    }
}

/// Replaces the names `bindings` gives values to in `expr`.
fn substitute(e: &mut SpannedExpr, bindings: &Bindings, diags: &mut Vec<Diagnostic>) {
    match e.0.as_mut() {
        Expr::Ref(path) if bindings.contains_key(&path.0[0]) => {
            let bound = &bindings[&path.0[0]];
            if path.0.len() == 1 {
                *e = bound.clone();
            } else if let Expr::Ref(prefix) = bound.0.as_ref() {
                let rest = path.0.split_off(1);
                path.0 = prefix.0.iter().cloned().chain(rest).collect();
            } else {
                diags.push(Diagnostic::new(
                    e.1,
                    format!(
                        "`{}` isn't given a name, so `{path}` names nothing",
                        path.0[0]
                    ),
                ));
            }
        }
        Expr::Call(callee, _) if bindings.contains_key(&callee.0[0]) => {
            diags.push(Diagnostic::new(e.1, "a parameter can't be called"));
        }
        expr => for_each_child(expr, &mut |x| substitute(x, bindings, diags)),
    }
}

/// Replaces the names `bindings` gives values to in `path`, which must be
/// given a path, like an account.
fn substitute_path(
    (path, span): &mut Spanned<Path>,
    bindings: &Bindings,
    diags: &mut Vec<Diagnostic>,
) {
    let Some(bound) = bindings.get(&path.0[0]) else {
        return;
    };
    match bound.0.as_ref() {
        Expr::Ref(prefix) => {
            let rest = path.0.split_off(1);
            path.0 = prefix.0.iter().cloned().chain(rest).collect();
        }
        _ => diags.push(
            Diagnostic::new(
                bound.1,
                format!(
                    "`{}` is used as an account, so it needs one, like `liabilities.loan`",
                    path.0[0]
                ),
            )
            .with_note(*span, "used as an account here"),
        ),
    }
}

fn substitute_decls(decls: &mut [Spanned<Decl>], bindings: &Bindings, diags: &mut Vec<Diagnostic>) {
    for (decl, _) in decls {
        let mut sub = |e: &mut SpannedExpr| substitute(e, bindings, diags);
        match decl {
            Decl::Let { body, .. } => match body {
                LetBody::Expr(e) => sub(e),
                LetBody::Arms(arms) => {
                    for arm in arms {
                        if let Some(when) = &mut arm.when {
                            sub(when);
                        }
                        sub(&mut arm.value);
                        if let Some(growth) = &mut arm.growth {
                            sub(&mut growth.by);
                        }
                    }
                }
            },
            Decl::Fn { body, .. } => {
                for stmt in body {
                    match stmt {
                        Stmt::Let { value, .. } | Stmt::Return(value) => {
                            substitute(value, bindings, diags)
                        }
                        Stmt::Mod(decls) => substitute_decls(decls, bindings, diags),
                    }
                }
            }
            Decl::Entry {
                when, after, body, ..
            } => {
                if let Some(when) = when {
                    substitute(when, bindings, diags);
                }
                for entry in after {
                    substitute_path(entry, bindings, diags);
                }
                substitute_stmts(body, bindings, diags);
            }
            Decl::Check { expr, when } => {
                sub(expr);
                if let Some(when) = when {
                    sub(when);
                }
            }
            Decl::Balances { balances, .. } => {
                for opening in balances {
                    substitute_path(&mut opening.target, bindings, diags);
                    substitute(&mut opening.value, bindings, diags);
                }
            }
            Decl::Mod { body, .. } => match body {
                ModBody::Inline(decls) => substitute_decls(decls, bindings, diags),
                ModBody::Call(call) => substitute(call, bindings, diags),
                ModBody::File => {}
            },
        }
    }
}

fn substitute_stmts(stmts: &mut [EntryStmt], bindings: &Bindings, diags: &mut Vec<Diagnostic>) {
    for stmt in stmts {
        match stmt {
            EntryStmt::Let { value, .. } => substitute(value, bindings, diags),
            EntryStmt::Move(m) => {
                substitute_path(&mut m.from, bindings, diags);
                substitute_path(&mut m.to, bindings, diags);
                substitute(&mut m.amount, bindings, diags);
            }
            EntryStmt::If {
                branches,
                otherwise,
            } => {
                for (cond, body) in branches {
                    substitute(cond, bindings, diags);
                    substitute_stmts(body, bindings, diags);
                }
                substitute_stmts(otherwise, bindings, diags);
            }
            EntryStmt::For { over, body, .. } => {
                for path in over {
                    substitute_path(path, bindings, diags);
                }
                substitute_stmts(body, bindings, diags);
            }
        }
    }
}

/// Adds the names `decls` declare, and the names declared inside their
/// functions and entries, to `out`.
fn collect_names<'a>(decls: &'a [Spanned<Decl>], out: &mut Vec<&'a Spanned<String>>) {
    for (decl, _) in decls {
        match decl {
            Decl::Let { name, .. } => out.push(name),
            Decl::Fn { name, params, .. } => {
                out.push(name);
                out.extend(params);
            }
            Decl::Entry { name, body, .. } => {
                out.push(name);
                EntryStmt::visit(body, &mut |stmt| match stmt {
                    EntryStmt::Let { name, .. } | EntryStmt::For { var: name, .. } => {
                        out.push(name)
                    }
                    EntryStmt::Move(_) | EntryStmt::If { .. } => {}
                });
            }
            Decl::Mod { name, body } => {
                out.push(name);
                if let ModBody::Inline(decls) = body {
                    collect_names(decls, out);
                }
            }
            Decl::Check { .. } | Decl::Balances { .. } => {}
        }
    }
}
