use crate::ast::Span;
use crate::sources::Sources;
use ariadne::{Color, Config, IndexType, Label, Report, ReportKind};
use chrono::NaiveDate;
use std::path::PathBuf;

#[derive(Debug, Clone)]
pub struct Diagnostic {
    pub message: String,
    pub span: Span,
    pub extra: Vec<(Span, String)>,
}

impl Diagnostic {
    pub fn new(span: Span, message: impl Into<String>) -> Self {
        Diagnostic {
            message: message.into(),
            span,
            extra: Vec::new(),
        }
    }

    pub fn with_note(mut self, span: Span, message: impl Into<String>) -> Self {
        self.extra.push((span, message.into()));
        self
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Severity {
    Error,
    Warning,
}

pub fn format_diagnostics(
    sources: &Sources,
    diags: &[Diagnostic],
    severity: Severity,
    color: bool,
) -> String {
    let (kind, label_color) = match severity {
        Severity::Error => (ReportKind::Error, Color::Red),
        Severity::Warning => (ReportKind::Warning, Color::Yellow),
    };
    let mut cache = ariadne::sources(sources.files().map(|f| (f.name.clone(), f.text.as_str())));
    let mut output = Vec::new();
    for d in diags {
        let Some((file, range)) = sources.locate(d.span) else {
            output.extend(format!("{kind}: {}\n", d.message).into_bytes());
            continue;
        };
        let config = Config::default()
            .with_index_type(IndexType::Byte)
            .with_color(color);
        // ariadne only underlines labels that carry a message, so the primary
        // label repeats the header.
        let mut builder = Report::build(kind, (file.name.clone(), range.clone()))
            .with_config(config)
            .with_message(&d.message)
            .with_label(
                Label::new((file.name.clone(), range))
                    .with_message(&d.message)
                    .with_color(label_color),
            );
        for (span, msg) in &d.extra {
            let Some((file, range)) = sources.locate(*span) else {
                continue;
            };
            builder = builder.with_label(
                Label::new((file.name.clone(), range))
                    .with_message(msg)
                    .with_color(Color::Yellow),
            );
        }
        let _ = builder.finish().write(&mut cache, &mut output);
    }
    String::from_utf8_lossy(&output).into_owned()
}

#[derive(Debug)]
pub enum Error {
    InvalidDateRange {
        from: NaiveDate,
        to: NaiveDate,
    },
    /// The file saldo was asked to run couldn't be read. Imports that can't be
    /// read are diagnostics instead, pointing at the import.
    Read {
        path: PathBuf,
        error: std::io::Error,
    },
    Diagnostic(Diagnostic),
}
