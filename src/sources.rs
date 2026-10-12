//! Reading a model's files: the one saldo runs and the files of its modules.

use crate::ast::{Change, Decl, ModBody, Program, Scenario, ScenarioBody, Span, Spanned};
use crate::errors::{self, Diagnostic, Error, Severity};
use std::collections::HashSet;
use std::ops::Range;
use std::path::{Path, PathBuf};

/// The text of every file in a model, for showing where errors are. Each file's
/// spans start after the previous file's, so a span alone says which file it's
/// in.
#[derive(Debug, Default)]
pub struct Sources {
    files: Vec<SourceFile>,
}

#[derive(Debug)]
pub(crate) struct SourceFile {
    /// How diagnostics name the file: the path saldo was given, or a module's
    /// file joined to the directory it's in.
    pub name: String,
    pub text: String,
    /// The span offset of the file's first byte.
    start: usize,
}

impl Sources {
    /// A single file whose spans start at 0.
    pub(crate) fn single(name: &str, text: &str) -> Self {
        let mut sources = Sources::default();
        sources.add(name.to_string(), text.to_string());
        sources
    }

    /// Adds a file, returning the offset its spans start at.
    fn add(&mut self, name: String, text: String) -> usize {
        // The gap keeps a span at the very end of one file from looking like
        // the start of the next.
        let start = self.files.last().map_or(0, |f| f.start + f.text.len() + 1);
        self.files.push(SourceFile { name, text, start });
        start
    }

    pub(crate) fn files(&self) -> impl Iterator<Item = &SourceFile> {
        self.files.iter()
    }

    /// The file `span` is in, and the span's byte range within it.
    pub(crate) fn locate(&self, span: Span) -> Option<(&SourceFile, Range<usize>)> {
        let i = self
            .files
            .partition_point(|f| f.start <= span.start)
            .checked_sub(1)?;
        let file = &self.files[i];
        let local = |offset: usize| offset.saturating_sub(file.start).min(file.text.len());
        let start = local(span.start);
        Some((file, start..local(span.end).max(start)))
    }

    /// Renders errors for display. `color` enables ANSI colors.
    pub fn format_errors(&self, errors: &[Error], color: bool) -> String {
        use std::fmt::Write;
        let mut out = String::new();
        for e in errors {
            match e {
                Error::InvalidDateRange { from, to } => {
                    writeln!(out, "--from ({from}) is after --to ({to})").ok();
                }
                Error::Read { path, error } => {
                    writeln!(out, "could not read `{}`: {error}", path.display()).ok();
                }
                Error::UnknownSeries { name, close } => {
                    write!(out, "`{name}` isn't an account, a group or a value").ok();
                    match close {
                        Some(close) => writeln!(out, "; did you mean `{close}`?"),
                        None => writeln!(out),
                    }
                    .ok();
                }
                Error::UnknownScenario { name, close } => {
                    write!(out, "the model has no scenario `{name}`").ok();
                    match close {
                        Some(close) => writeln!(out, "; did you mean `{close}`?"),
                        None => writeln!(out),
                    }
                    .ok();
                }
                Error::Diagnostic(d) => {
                    out.push_str(&errors::format_diagnostics(
                        self,
                        std::slice::from_ref(d),
                        Severity::Error,
                        color,
                    ));
                }
            }
        }
        out
    }

    /// Renders warnings for display. `color` enables ANSI colors.
    pub fn format_warnings(&self, warnings: &[Diagnostic], color: bool) -> String {
        errors::format_diagnostics(self, warnings, Severity::Warning, color)
    }
}

/// Reads the model in the file at `path` and the files of its modules into
/// one program, adding their text to `sources`.
pub(crate) fn load_file(sources: &mut Sources, path: &Path) -> Result<Program, Vec<Error>> {
    let read_error = |error| {
        vec![Error::Read {
            path: path.to_path_buf(),
            error,
        }]
    };
    let canonical = std::fs::canonicalize(path).map_err(read_error)?;
    let text = std::fs::read_to_string(path).map_err(read_error)?;
    let mut loader = Loader {
        sources,
        seen: HashSet::from([canonical]),
        scenarios: Vec::new(),
        diags: Vec::new(),
    };
    let program = loader.read_model(path.display().to_string(), text, path.parent());
    loader.finish(program)
}

/// Reads a model given as text, which can't have modules in other files
/// because it isn't in a directory.
pub(crate) fn load_text(sources: &mut Sources, text: &str) -> Result<Program, Vec<Error>> {
    let mut loader = Loader {
        sources,
        seen: HashSet::new(),
        scenarios: Vec::new(),
        diags: Vec::new(),
    };
    let program = loader.read_model(String::new(), text.to_string(), None);
    loader.finish(program)
}

/// Reads the scenario in the file at `path`, given to saldo rather than
/// declared in the model, as if `scenario <stem>` at the top of the model read
/// it: the modules it declares are in the directory named after the file, next
/// to it. It's named `name`.
pub(crate) fn load_scenario_file(
    sources: &mut Sources,
    path: &Path,
    name: &str,
) -> Result<Scenario, Vec<Error>> {
    let read_error = |error| {
        vec![Error::Read {
            path: path.to_path_buf(),
            error,
        }]
    };
    let text = std::fs::read_to_string(path).map_err(read_error)?;
    let stem = path
        .file_stem()
        .map_or_else(String::new, |s| s.to_string_lossy().into_owned());
    let mut loader = Loader {
        sources,
        seen: HashSet::new(),
        scenarios: Vec::new(),
        diags: Vec::new(),
    };
    let changes = loader
        .parse(
            path.display().to_string(),
            text,
            crate::parser::parse_changes,
        )
        .unwrap_or_default();
    let start = loader.sources.files.last().expect("just added").start;
    // Errors the scenario causes elsewhere note its name, at the file's start.
    let scenario = Scenario {
        name: (stem, Span::new(start, start)),
        module: Vec::new(),
        body: ScenarioBody::Inline(changes),
    };
    loader.load_scenario(scenario, path.parent(), &[]);
    let mut scenario = loader.scenarios.pop().expect("just loaded");
    scenario.name.0 = name.to_string();
    loader.finish(scenario)
}

struct Loader<'a> {
    sources: &'a mut Sources,
    /// The canonical paths of the files read so far, so a file isn't read as
    /// two modules.
    seen: HashSet<PathBuf>,
    /// The scenarios read so far, each with the changes of every block with
    /// its name, in the modules they're declared in.
    scenarios: Vec<Scenario>,
    diags: Vec<Diagnostic>,
}

impl Loader<'_> {
    /// The model in `text`, the file saldo runs, with each `mod x` and
    /// `scenario x` replaced by what's in its file. `dir` is where their files
    /// are.
    fn read_model(&mut self, name: String, text: String, dir: Option<&Path>) -> Program {
        let decls = self.read(name, text, dir, Some(&[]));
        Program {
            decls,
            scenarios: std::mem::take(&mut self.scenarios),
        }
    }

    /// The declarations in `text`, the file of module `module`, with each
    /// `mod x` replaced by the declarations in its file, and its scenarios
    /// read. `dir` is where the files of its modules are. A module in a
    /// scenario has no `module`, since it can't declare scenarios.
    fn read(
        &mut self,
        name: String,
        text: String,
        dir: Option<&Path>,
        module: Option<&[Spanned<String>]>,
    ) -> Vec<Spanned<Decl>> {
        let parse = match module {
            Some(_) => crate::parser::parse,
            None => crate::parser::parse_in_scenario,
        };
        let Some(program) = self.parse(name, text, parse) else {
            return Vec::new();
        };
        if let Some(module) = module {
            for scenario in program.scenarios {
                self.load_scenario(scenario, dir, module);
            }
        }
        self.load_modules(program.decls, dir, module)
    }

    /// Adds `text`, the file `name`, to the sources, and parses it with `parse`.
    fn parse<T>(
        &mut self,
        name: String,
        text: String,
        parse: impl FnOnce(Vec<(crate::lexer::Token, Span)>) -> Result<T, Vec<Diagnostic>>,
    ) -> Option<T> {
        let start = self.sources.add(name, text);
        let text = &self.sources.files.last().expect("just added").text;
        match crate::lexer::lex(text, start).and_then(parse) {
            Ok(parsed) => Some(parsed),
            Err(diags) => {
                self.diags.extend(diags);
                None
            }
        }
    }

    /// Reads the files of the modules in `decls`, as Rust does: `mod x` in a
    /// file reads `x.saldo` from `dir`, and the modules in `x`, written in it
    /// or in their own files, are in `dir/x`.
    /// `module` is the path of the module they're in, as [`Loader::read`]
    /// takes it.
    fn load_modules(
        &mut self,
        decls: Vec<Spanned<Decl>>,
        dir: Option<&Path>,
        module: Option<&[Spanned<String>]>,
    ) -> Vec<Spanned<Decl>> {
        decls
            .into_iter()
            .map(|decl| self.load_module(decl, dir, module))
            .collect()
    }

    /// `decl`, in `module`, with what's in its file if it's `mod x`, and in
    /// the files of the modules in it if it's a module.
    fn load_module(
        &mut self,
        (decl, span): Spanned<Decl>,
        dir: Option<&Path>,
        module: Option<&[Spanned<String>]>,
    ) -> Spanned<Decl> {
        let Decl::Mod { name, body } = decl else {
            return (decl, span);
        };
        let sub = dir.map(|dir| dir.join(&name.0));
        let inner = module.map(|module| [module, std::slice::from_ref(&name)].concat());
        let inner = inner.as_deref();
        let decls = match body {
            ModBody::Inline(decls) => self.load_modules(decls, sub.as_deref(), inner),
            ModBody::File => self
                .read_file(dir, &name, "mod")
                .map(|(path, text)| self.read(path, text, sub.as_deref(), inner))
                .unwrap_or_default(),
            // Made by a function, which the resolver expands.
            call @ ModBody::Call(_) => return (Decl::Mod { name, body: call }, span),
        };
        let body = ModBody::Inline(decls);
        (Decl::Mod { name, body }, span)
    }

    /// Reads `scenario`, declared in a file of module `module`, with what's
    /// in its file if it's `scenario x`, and in the files of the modules it
    /// declares, which are in `x` next to it, laid out as the model's are in
    /// `dir`. Its changes go in the module it's in, and are added to those of
    /// the scenarios read with its name.
    fn load_scenario(
        &mut self,
        scenario: Scenario,
        dir: Option<&Path>,
        module: &[Spanned<String>],
    ) {
        let Scenario {
            name,
            module: in_file,
            body,
        } = scenario;
        let dir = dir.map(|dir| {
            in_file
                .iter()
                .fold(dir.to_path_buf(), |d, (m, _)| d.join(m))
        });
        let changes = match body {
            ScenarioBody::Inline(changes) => changes,
            ScenarioBody::File => self
                .read_file(dir.as_deref(), &name, "scenario")
                .and_then(|(path, text)| self.parse(path, text, crate::parser::parse_changes))
                .unwrap_or_default(),
        };
        let sub = dir.map(|dir| dir.join(&name.0));
        let here = [module, &in_file].concat();
        let changes = changes.into_iter().map(|(change, span)| {
            let change = match change {
                Change::Decl { module, decl } => {
                    let dir = sub
                        .as_ref()
                        .map(|sub| module.iter().fold(sub.clone(), |d, (m, _)| d.join(m)));
                    let (decl, _) = self.load_module((decl, span), dir.as_deref(), None);
                    let module = [&here[..], &module].concat();
                    Change::Decl { module, decl }
                }
                Change::Drop { module, path, when } => Change::Drop {
                    module: [&here[..], &module].concat(),
                    path,
                    when,
                },
            };
            (change, span)
        });
        let changes: Vec<_> = changes.collect();
        match self.scenarios.iter_mut().find(|s| s.name.0 == name.0) {
            Some(Scenario {
                body: ScenarioBody::Inline(merged),
                ..
            }) => merged.extend(changes),
            Some(_) => unreachable!("scenarios read have their changes"),
            None => self.scenarios.push(Scenario {
                name,
                module: Vec::new(),
                body: ScenarioBody::Inline(changes),
            }),
        }
    }

    /// The path and text of `name.saldo` in `dir`, which `<keyword> name`
    /// reads, unless it can't be read.
    fn read_file(
        &mut self,
        dir: Option<&Path>,
        (name, span): &Spanned<String>,
        keyword: &str,
    ) -> Option<(String, String)> {
        let Some(dir) = dir else {
            self.diags.push(Diagnostic::new(
                *span,
                format!("`{keyword} {name}` reads `{name}.saldo`, which only works in a model read from a file"),
            ));
            return None;
        };
        let path = dir.join(format!("{name}.saldo"));
        let read_error = |error: std::io::Error| {
            Diagnostic::new(
                *span,
                format!("could not read `{}`: {error}", path.display()),
            )
        };
        let canonical = match std::fs::canonicalize(&path) {
            Ok(canonical) => canonical,
            Err(e) => {
                self.diags.push(read_error(e));
                return None;
            }
        };
        if !self.seen.insert(canonical) {
            self.diags.push(Diagnostic::new(
                *span,
                format!("`{}` is already read as another module", path.display()),
            ));
            return None;
        }
        match std::fs::read_to_string(&path) {
            Ok(text) => Some((path.display().to_string(), text)),
            Err(e) => {
                self.diags.push(read_error(e));
                None
            }
        }
    }

    fn finish<T>(self, loaded: T) -> Result<T, Vec<Error>> {
        if self.diags.is_empty() {
            Ok(loaded)
        } else {
            Err(self.diags.into_iter().map(Error::Diagnostic).collect())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spans_locate_their_file() {
        let mut sources = Sources::default();
        assert!(sources.locate(Span::new(0, 0)).is_none());
        assert_eq!(sources.add("a".into(), "abc".into()), 0);
        assert_eq!(sources.add("b".into(), "".into()), 4);
        assert_eq!(sources.add("c".into(), "de".into()), 5);
        let locate = |start, end| {
            let (file, range) = sources.locate(Span::new(start, end)).unwrap();
            (file.name.as_str(), range)
        };
        assert_eq!(locate(0, 2), ("a", 0..2));
        // The end of a file is still in it, not in the next one.
        assert_eq!(locate(3, 3), ("a", 3..3));
        assert_eq!(locate(4, 4), ("b", 0..0));
        assert_eq!(locate(5, 7), ("c", 0..2));
        assert_eq!(locate(6, 9), ("c", 1..2));
    }
}
