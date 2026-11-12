//! Source spans and diagnostics shared by every compiler stage.

use std::fmt;

/// A half-open byte range `[start, end)` into the source text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Span {
    pub start: u32,
    pub end: u32,
}

impl Span {
    pub fn new(start: usize, end: usize) -> Self {
        Span { start: start as u32, end: end as u32 }
    }

    /// The smallest span covering both `self` and `other`.
    pub fn join(self, other: Span) -> Span {
        Span {
            start: self.start.min(other.start),
            end: self.end.max(other.end),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Diagnostic {
    pub message: String,
    pub span: Span,
}

impl Diagnostic {
    pub fn new(message: impl Into<String>, span: Span) -> Self {
        Diagnostic { message: message.into(), span }
    }

    /// Renders `file:line:col: error: message` plus the offending line and a caret.
    pub fn render(&self, file: &str, source: &str) -> String {
        let mut start = (self.span.start as usize).min(source.len());
        while !source.is_char_boundary(start) {
            start -= 1;
        }
        let before = &source[..start];
        let line = before.matches('\n').count() + 1;
        let line_start = before.rfind('\n').map_or(0, |i| i + 1);
        let col = source[line_start..start].chars().count() + 1;
        let line_end = source[start..].find('\n').map_or(source.len(), |i| start + i);
        let text = &source[line_start..line_end];
        format!(
            "{file}:{line}:{col}: error: {}\n  {text}\n  {}^",
            self.message,
            " ".repeat(col - 1)
        )
    }
}

impl fmt::Display for Diagnostic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for Diagnostic {}

pub type Result<T> = std::result::Result<T, Diagnostic>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_line_and_column() {
        let d = Diagnostic::new("boom", Span::new(4, 5));
        assert_eq!(d.render("a.js", "ab\ncd\nef"), "a.js:2:2: error: boom\n  cd\n   ^");
    }

    #[test]
    fn join_covers_both() {
        assert_eq!(Span::new(2, 4).join(Span::new(1, 3)), Span::new(1, 4));
    }
}
