//! Applies scenarios to a model before `mod x = f(…)` is expanded: replaces
//! the declarations they replace and adds the ones they add, and hands the
//! resolver the rest, like what they drop.

use crate::ast::{
    Change, Decl, Expr, LetBody, ModBody, Path, Program, Scenario, ScenarioBody, Span, Spanned,
    SpannedExpr, Stmt,
};
use crate::errors::Diagnostic;
use std::collections::{HashMap, HashSet};

/// The name of the scenario that changes nothing.
pub const BASE: &str = "base";

/// What scenarios change that the resolver needs to know.
#[derive(Clone, Debug, Default)]
pub struct Applied {
    /// What they drop, by its path from the top of the model, and on which
    /// days, or every day.
    pub drops: Vec<(Spanned<Path>, Option<SpannedExpr>)>,
    /// The values and functions they add, by full name, and where they're
    /// named. Nothing reading one is most likely a misspelled replacement.
    pub added: Vec<(String, Span)>,
    /// The names that the draws of some declarations are keyed by, by those
    /// declarations' full names, so that a declaration that's replaced keeps
    /// its draws.
    pub draw_keys: HashMap<String, String>,
    /// The `balances` blocks they add, by span, whose openings and totals so
    /// far replace the model's.
    pub balances: HashSet<Span>,
}

/// Reports scenarios that can't have their names: two with one name, one
/// named `base`, and one named like a declaration at the top of the model.
pub fn check_names(program: &Program) -> Vec<Diagnostic> {
    let mut diags = Vec::new();
    let mut seen: HashMap<&str, Span> = program
        .decls
        .iter()
        .filter_map(|(decl, _)| decl_name(decl))
        .map(|(name, span)| (name.as_str(), *span))
        .collect();
    for scenario in &program.scenarios {
        let (name, span) = &scenario.name;
        if name == BASE {
            diags.push(Diagnostic::new(
                *span,
                "`base` is the model without a scenario, so it can't name one",
            ));
        } else if let Some(previous) = seen.get(name.as_str()) {
            diags.push(
                Diagnostic::new(*span, format!("`{name}` is already declared"))
                    .with_note(*previous, "declared here"),
            );
        } else {
            seen.insert(name, *span);
        }
    }
    diags
}

/// Whether `span` is in what `scenario` changes, in the file saldo runs or its
/// own.
pub fn contains(scenario: &Scenario, span: Span) -> bool {
    let ScenarioBody::Inline(changes) = &scenario.body else {
        return false;
    };
    changes
        .iter()
        .any(|(_, change)| change.start <= span.start && span.end <= change.end)
}

/// `program` with the changes `scenarios` make, one after another, and what
/// the resolver needs to know about them.
pub fn apply(
    program: &Program,
    scenarios: &[&Scenario],
) -> Result<(Program, Applied), Vec<Diagnostic>> {
    let mut applier = Applier {
        decls: program.decls.clone(),
        applied: Applied::default(),
        changed: Vec::new(),
        diags: Vec::new(),
    };
    for scenario in scenarios {
        let ScenarioBody::Inline(changes) = &scenario.body else {
            unreachable!("loading reads scenarios' files")
        };
        for (change, span) in changes {
            match change {
                Change::Drop { path, when } => {
                    applier.applied.drops.push((path.clone(), when.clone()));
                }
                Change::Decl {
                    decl: decl @ Decl::Check { .. },
                    ..
                } => applier.decls.push((decl.clone(), *span)),
                Change::Decl {
                    decl: decl @ Decl::Balances { .. },
                    ..
                } => {
                    applier.applied.balances.insert(*span);
                    applier.decls.push((decl.clone(), *span));
                }
                Change::Decl { module, decl } => {
                    if let Err(diag) = applier.declare(module, decl, *span, &scenario.name.0) {
                        applier.diags.push(diag);
                    }
                }
            }
        }
    }
    if !applier.diags.is_empty() {
        return Err(applier.diags);
    }
    let program = Program {
        decls: applier.decls,
        scenarios: Vec::new(),
    };
    Ok((program, applier.applied))
}

struct Applier {
    decls: Vec<Spanned<Decl>>,
    applied: Applied,
    /// The paths declarations have been changed at so far, and where.
    changed: Vec<(Vec<String>, Span)>,
    diags: Vec<Diagnostic>,
}

/// What kind of declaration a scenario can replace, by the keyword that
/// declares it.
fn keyword(decl: &Decl) -> &'static str {
    match decl {
        Decl::Let { .. } => "let",
        Decl::Fn { .. } => "fn",
        Decl::Entry { .. } => "entry",
        Decl::Mod { .. } => "mod",
        Decl::Check { .. } => "check",
        Decl::Balances { .. } => "balances",
    }
}

fn decl_name(decl: &Decl) -> Option<&Spanned<String>> {
    match decl {
        Decl::Let { name, .. }
        | Decl::Fn { name, .. }
        | Decl::Entry { name, .. }
        | Decl::Mod { name, .. } => Some(name),
        Decl::Check { .. } | Decl::Balances { .. } => None,
    }
}

impl Applier {
    /// `decl`, written at `span` in `scenario`, in `module`: it replaces the
    /// declaration with its name there, if there is one, and is added if not.
    fn declare(
        &mut self,
        module: &[Spanned<String>],
        decl: &Decl,
        span: Span,
        scenario: &str,
    ) -> Result<(), Diagnostic> {
        let (name, name_span) = decl_name(decl).expect("checks and balances are added");
        let module_path: Vec<String> = module.iter().map(|(part, _)| part.clone()).collect();
        let path: Vec<String> = module_path.iter().cloned().chain([name.clone()]).collect();
        self.check_unchanged(&path, *name_span)?;
        let decls = module_decls(&mut self.decls, module)?;
        let full = path.join(".");
        let Some(i) = decls
            .iter()
            .position(|(d, _)| decl_name(d).is_some_and(|(n, _)| n == name))
        else {
            if matches!(decl, Decl::Let { .. })
                || matches!(decl, Decl::Fn { body, .. } if !makes_module(body))
            {
                self.applied.added.push((full, *name_span));
            }
            decls.push((decl.clone(), span));
            return Ok(());
        };
        let (old, _) = &decls[i];
        if keyword(old) != keyword(decl) {
            return Err(Diagnostic::new(
                *name_span,
                format!(
                    "`{full}` is declared with `{}`, so a scenario replaces it with `{0}`",
                    keyword(old)
                ),
            ));
        }
        // A value or a function that reads its own name reads the one it
        // replaces, which stays, under a name no one can write.
        let mut decl = decl.clone();
        let reads_old = match &mut decl {
            Decl::Fn { body, .. } if makes_module(body) => false,
            Decl::Let { .. } | Decl::Fn { .. } => {
                let names: HashSet<String> = decls
                    .iter()
                    .filter_map(|(d, _)| decl_name(d).map(|(n, _)| n.clone()))
                    .collect();
                let own = Own {
                    path: &path,
                    names: &names,
                };
                rename_reads(&mut decl, &own)
            }
            _ => false,
        };
        if reads_old {
            let hidden = hidden(name);
            match &mut decls[i].0 {
                Decl::Let { name, .. } | Decl::Fn { name, .. } => name.0 = hidden.clone(),
                _ => unreachable!("only values and functions read what they replace"),
            }
            let hidden_full = module_path
                .iter()
                .cloned()
                .chain([hidden])
                .collect::<Vec<_>>();
            self.applied
                .draw_keys
                .insert(hidden_full.join("."), full.clone());
            self.applied
                .draw_keys
                .insert(full.clone(), format!("{full}${scenario}"));
            decls.insert(i + 1, (decl, span));
        } else {
            decls[i] = (decl, span);
        }
        Ok(())
    }

    /// Reports a change at `path` if an earlier one changed it, or a module
    /// it's in, or something in it.
    fn check_unchanged(&mut self, path: &[String], span: Span) -> Result<(), Diagnostic> {
        let full = path.join(".");
        for (other, other_span) in &self.changed {
            let other_full = other.join(".");
            let message = if other.as_slice() == path {
                format!("`{full}` is already changed")
            } else if path.starts_with(other) {
                format!("`{full}` is in `{other_full}`, which is already changed")
            } else if other.starts_with(path) {
                format!("`{full}` has `{other_full}` in it, which is already changed")
            } else {
                continue;
            };
            return Err(Diagnostic::new(span, message).with_note(*other_span, "changed here"));
        }
        self.changed.push((path.to_vec(), span));
        Ok(())
    }
}

/// The name a declaration that's replaced, but read by its replacement, keeps.
fn hidden(name: &str) -> String {
    format!("{name}$")
}

fn makes_module(body: &[Stmt]) -> bool {
    matches!(body.last(), Some(Stmt::Mod(_)))
}

/// The declarations of `module`, which must be written in the model, not made
/// by a function.
fn module_decls<'d>(
    decls: &'d mut Vec<Spanned<Decl>>,
    module: &[Spanned<String>],
) -> Result<&'d mut Vec<Spanned<Decl>>, Diagnostic> {
    let mut decls = decls;
    for (i, (part, span)) in module.iter().enumerate() {
        let so_far = module[..=i]
            .iter()
            .map(|(part, _)| part.as_str())
            .collect::<Vec<_>>()
            .join(".");
        let Some(pos) = decls
            .iter()
            .position(|(d, _)| decl_name(d).is_some_and(|(n, _)| n == part))
        else {
            let modules = decls.iter().filter_map(|(d, _)| match d {
                Decl::Mod { name, .. } => Some(name.0.as_str()),
                _ => None,
            });
            let mut diag = Diagnostic::new(*span, format!("the model has no module `{so_far}`"));
            if let Some(close) = crate::util::closest(part, modules) {
                diag = diag.with_note(*span, format!("did you mean `{close}`?"));
            }
            return Err(diag);
        };
        let current = decls;
        decls = match &mut current[pos].0 {
            Decl::Mod {
                body: ModBody::Inline(inner),
                ..
            } => inner,
            Decl::Mod {
                body: ModBody::Call(_),
                ..
            } => {
                return Err(Diagnostic::new(
                    *span,
                    format!(
                        "`{so_far}` is made by a function, so a scenario replaces it whole, \
                         with `mod {so_far} = …`"
                    ),
                ));
            }
            Decl::Mod {
                body: ModBody::File,
                ..
            } => unreachable!("loading reads modules' files"),
            _ => return Err(Diagnostic::new(*span, format!("`{so_far}` isn't a module"))),
        };
    }
    Ok(decls)
}

/// A declaration that replaces another: its path from the top of the model,
/// and the names declared in its module.
struct Own<'a> {
    path: &'a [String],
    names: &'a HashSet<String>,
}

impl Own<'_> {
    /// Whether `path`, read in the declaration where `locals` are in scope,
    /// names the declaration, as the resolver would look it up.
    fn is_named_by(&self, path: &[String], locals: &[String]) -> bool {
        let (name, module) = self.path.split_last().expect("paths aren't empty");
        match path.split_first() {
            Some((root, rest)) if root == Path::ROOT => rest == self.path,
            Some((first, _)) if locals.contains(first) => false,
            Some((first, _)) if self.names.contains(first) => path == [name.clone()],
            _ => !module.is_empty() && path == self.path,
        }
    }
}

/// Makes `decl`'s reads of its own name read the declaration it replaces,
/// under its hidden name. Returns whether there were any.
fn rename_reads(decl: &mut Decl, own: &Own) -> bool {
    let mut renamed = false;
    match decl {
        Decl::Let { body, .. } => {
            let exprs: Vec<&mut SpannedExpr> = match body {
                LetBody::Expr(e) => vec![e],
                LetBody::Arms(arms) => arms
                    .iter_mut()
                    .flat_map(|arm| {
                        arm.when
                            .iter_mut()
                            .chain([&mut arm.value])
                            .chain(arm.growth.as_mut().map(|g| &mut g.by))
                    })
                    .collect(),
            };
            for e in exprs {
                rename_in(e, own, &[], &mut renamed);
            }
        }
        Decl::Fn { params, body, .. } => {
            let mut locals: Vec<String> = params.iter().map(|(p, _)| p.clone()).collect();
            for stmt in body {
                match stmt {
                    Stmt::Let { name, value } => {
                        rename_in(value, own, &locals, &mut renamed);
                        locals.push(name.clone());
                    }
                    Stmt::Return(e) => rename_in(e, own, &locals, &mut renamed),
                    Stmt::Mod(_) => unreachable!("functions that make modules are replaced whole"),
                }
            }
        }
        _ => unreachable!("only values and functions read what they replace"),
    }
    renamed
}

fn rename_in(e: &mut SpannedExpr, own: &Own, locals: &[String], renamed: &mut bool) {
    match e.0.as_mut() {
        Expr::Ref(path) | Expr::Call(path, _) if own.is_named_by(&path.0, locals) => {
            let last = path.0.last_mut().expect("paths aren't empty");
            *last = hidden(last);
            *renamed = true;
        }
        _ => {}
    }
    crate::expand::for_each_child(e.0.as_mut(), &mut |child| {
        rename_in(child, own, locals, renamed)
    });
}
