use crate::ast::Span;
use crate::lexer::Token;
use ariadne::{Color, Config, IndexType, Label, Report, ReportKind};
use chumsky::error::Rich;

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

    pub fn from_lex_err(err: Rich<'_, char>) -> Self {
        Diagnostic {
            message: err.to_string(),
            span: *err.span(),
            extra: Vec::new(),
        }
    }

    pub fn from_parse_err<'src>(err: Rich<'src, Token<'src>>) -> Self {
        Diagnostic {
            message: err.to_string(),
            span: *err.span(),
            extra: Vec::new(),
        }
    }
}

pub fn report(path: &str, source: &str, diags: &[Diagnostic]) {
    for d in diags {
        let range = d.span.into_range();
        let mut builder = Report::build(ReportKind::Error, (path, range.clone()))
            .with_config(Config::default().with_index_type(IndexType::Byte))
            .with_message(&d.message)
            .with_label(
                Label::new((path, range))
                    .with_message(&d.message)
                    .with_color(Color::Red),
            );
        for (span, msg) in &d.extra {
            builder = builder.with_label(
                Label::new((path, span.into_range()))
                    .with_message(msg)
                    .with_color(Color::Yellow),
            );
        }
        let _ = builder
            .finish()
            .eprint((path, ariadne::Source::from(source)));
    }
}
