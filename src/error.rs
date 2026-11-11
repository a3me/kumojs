//! Helpers that build `CompileError`s (the error type itself lives in the crate root).

use crate::*;

pub(crate) fn unsupported<T>(what: impl Into<String>, span: Span) -> CompileResult<T> {
    Err(CompileError::Unsupported {
        what: what.into(),
        span,
        location: String::new(),
    })
}

pub(crate) fn format_location(cm: &SourceMap, span: Span) -> String {
    let loc = cm.lookup_char_pos(span.lo);
    format!(" at {}:{}:{}", loc.file.name, loc.line, loc.col.0 + 1)
}
