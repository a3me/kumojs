//! Compile errors, and helpers that build them.

use swc_common::{SourceMap, Span};
use thiserror::Error;

#[derive(Error, Debug)]
pub enum CompileError {
    #[error("failed to load file: {0}")]
    LoadFile(String),
    #[error("parse error: {message}{}", at(location))]
    Parse {
        message: String,
        location: Option<Location>,
    },
    #[error("too many locals, max number of locals supported is 256")]
    TooManyLocals,
    #[error("unsupported {what}{}", at(location))]
    Unsupported {
        what: String,
        span: Span,
        location: Option<Location>,
    },
    /// an early error the parser doesn't check (e.g. redeclaring a `let`)
    #[error("syntax error: {what}{}", at(location))]
    Syntax {
        what: String,
        span: Span,
        location: Option<Location>,
    },
}

/// Where in the source an error is (1-based line and column).
#[derive(Debug, Clone)]
pub struct Location {
    pub file: String,
    pub line: usize,
    pub column: usize,
}

impl Location {
    pub(crate) fn of(cm: &SourceMap, span: Span) -> Self {
        let loc = cm.lookup_char_pos(span.lo);
        Location {
            file: loc.file.name.to_string(),
            line: loc.line,
            column: loc.col.0 + 1,
        }
    }
}

fn at(location: &Option<Location>) -> String {
    match location {
        Some(l) => format!(" at {}:{}:{}", l.file, l.line, l.column),
        None => String::new(),
    }
}

impl CompileError {
    /// Resolve the error's span (if it has one) to a source location.
    pub(crate) fn locate(mut self, cm: &SourceMap) -> Self {
        if let CompileError::Unsupported { span, location, .. }
        | CompileError::Syntax { span, location, .. } = &mut self
        {
            *location = Some(Location::of(cm, *span));
        }
        self
    }
}

pub(crate) type CompileResult<T = ()> = Result<T, CompileError>;

pub(crate) fn unsupported<T>(what: impl Into<String>, span: Span) -> CompileResult<T> {
    Err(CompileError::Unsupported {
        what: what.into(),
        span,
        location: None,
    })
}

pub(crate) fn syntax_error<T>(what: impl Into<String>, span: Span) -> CompileResult<T> {
    Err(CompileError::Syntax {
        what: what.into(),
        span,
        location: None,
    })
}
