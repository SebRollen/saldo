//! Reading a model's files: the one saldo runs and the files of its modules.

use crate::ast::{Decl, ModBody, Program, Span, Spanned};
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
        diags: Vec::new(),
    };
    let decls = loader.read(path.display().to_string(), text, path.parent());
    loader.finish(decls)
}

/// Reads a model given as text, which can't have modules in other files
/// because it isn't in a directory.
pub(crate) fn load_text(sources: &mut Sources, text: &str) -> Result<Program, Vec<Error>> {
    let mut loader = Loader {
        sources,
        seen: HashSet::new(),
        diags: Vec::new(),
    };
    let decls = loader.read(String::new(), text.to_string(), None);
    loader.finish(decls)
}

struct Loader<'a> {
    sources: &'a mut Sources,
    /// The canonical paths of the files read so far, so a file isn't read as
    /// two modules.
    seen: HashSet<PathBuf>,
    diags: Vec<Diagnostic>,
}

impl Loader<'_> {
    /// The declarations in `text`, with each `mod x` replaced by the
    /// declarations in its file. `dir` is where the files of its modules are.
    fn read(&mut self, name: String, text: String, dir: Option<&Path>) -> Vec<Spanned<Decl>> {
        let start = self.sources.add(name, text);
        let text = &self.sources.files.last().expect("just added").text;
        match crate::lexer::lex(text, start).and_then(crate::parser::parse) {
            Ok(program) => self.load_modules(program.decls, dir),
            Err(diags) => {
                self.diags.extend(diags);
                Vec::new()
            }
        }
    }

    /// Reads the files of the modules in `decls`, as Rust does: `mod x` in a
    /// file reads `x.saldo` from `dir`, and the modules in `x`, written in it
    /// or in their own files, are in `dir/x`.
    fn load_modules(
        &mut self,
        decls: Vec<Spanned<Decl>>,
        dir: Option<&Path>,
    ) -> Vec<Spanned<Decl>> {
        decls
            .into_iter()
            .map(|(decl, span)| {
                let Decl::Mod { name, body } = decl else {
                    return (decl, span);
                };
                let sub = dir.map(|dir| dir.join(&name.0));
                let decls = match body {
                    ModBody::Inline(decls) => self.load_modules(decls, sub.as_deref()),
                    ModBody::File => self.load_file(dir, &name, sub.as_deref()),
                };
                let body = ModBody::Inline(decls);
                (Decl::Mod { name, body }, span)
            })
            .collect()
    }

    fn load_file(
        &mut self,
        dir: Option<&Path>,
        (name, span): &Spanned<String>,
        sub: Option<&Path>,
    ) -> Vec<Spanned<Decl>> {
        let Some(dir) = dir else {
            self.diags.push(Diagnostic::new(
                *span,
                format!("`mod {name}` reads `{name}.saldo`, which only works in a model read from a file"),
            ));
            return Vec::new();
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
                return Vec::new();
            }
        };
        if !self.seen.insert(canonical) {
            self.diags.push(Diagnostic::new(
                *span,
                format!("`{}` is already read as another module", path.display()),
            ));
            return Vec::new();
        }
        match std::fs::read_to_string(&path) {
            Ok(text) => self.read(path.display().to_string(), text, sub),
            Err(e) => {
                self.diags.push(read_error(e));
                Vec::new()
            }
        }
    }

    fn finish(self, decls: Vec<Spanned<Decl>>) -> Result<Program, Vec<Error>> {
        if self.diags.is_empty() {
            Ok(Program { decls })
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
