use crate::parsing::ast::Span;
use std::collections::HashMap;
use std::fmt;
use std::path::PathBuf;
use std::sync::Arc;

#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceType {
    Path(Arc<PathBuf>),
    Dependency(String),
    Volatile,
}

impl fmt::Display for SourceType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SourceType::Path(path) => write!(f, "{}", path.display()),
            SourceType::Dependency(id) => write!(f, "{id}"),
            SourceType::Volatile => write!(f, "volatile"),
        }
    }
}

impl SourceType {
    /// Decode a binding-layer source label (JS/Hex/Java `load` labeled form) into a [`SourceType`].
    pub fn from_binding_label(label: &str) -> Result<Self, String> {
        if label.trim().is_empty() {
            return Err("source label must be non-empty".to_string());
        }
        if label.starts_with('@') {
            return Ok(SourceType::Dependency(label.to_string()));
        }
        Ok(SourceType::Path(Arc::new(PathBuf::from(label))))
    }
}

/// Positional source location: source id and span.
///
/// Text is resolved via `text_from(sources)` when needed. No embedded source text.
#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize)]
pub struct Source {
    /// Source identifier (e.g. filesystem path label)
    pub source_type: SourceType,

    /// Span in source code
    pub span: Span,
}

impl Source {
    #[must_use]
    pub fn new(source_type: SourceType, span: Span) -> Self {
        Self { source_type, span }
    }

    /// Resolve source text from the sources map.
    #[must_use]
    pub fn text_from<'a>(
        &self,
        sources: &'a HashMap<SourceType, String>,
    ) -> Option<std::borrow::Cow<'a, str>> {
        let s = sources.get(&self.source_type)?;
        s.get(self.span.start..self.span.end)
            .map(std::borrow::Cow::Borrowed)
    }

    /// Extract the source text from the given source string.
    ///
    /// Returns `None` if the span is out of bounds.
    #[must_use]
    pub fn extract_text(&self, source: &str) -> Option<String> {
        let bytes = source.as_bytes();
        if self.span.start < bytes.len() && self.span.end <= bytes.len() {
            Some(String::from_utf8_lossy(&bytes[self.span.start..self.span.end]).to_string())
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn test_extract_text_valid() {
        let source = "hello world";
        let span = Span {
            start: 0,
            end: 5,
            line: 1,
            col: 0,
        };
        let loc = Source::new(SourceType::Volatile, span);
        assert_eq!(loc.extract_text(source), Some("hello".to_string()));
    }

    #[test]
    fn test_extract_text_full_string() {
        let source = "hello world";
        let span = Span {
            start: 0,
            end: 11,
            line: 1,
            col: 0,
        };
        let loc = Source::new(SourceType::Volatile, span);
        assert_eq!(loc.extract_text(source), Some("hello world".to_string()));
    }

    #[test]
    fn test_extract_text_empty() {
        let source = "hello world";
        let span = Span {
            start: 5,
            end: 5,
            line: 1,
            col: 5,
        };
        let loc = Source::new(SourceType::Volatile, span);
        assert_eq!(loc.extract_text(source), Some("".to_string()));
    }

    #[test]
    fn test_extract_text_out_of_bounds_start() {
        let source = "hello";
        let span = Span {
            start: 10,
            end: 15,
            line: 1,
            col: 10,
        };
        let loc = Source::new(SourceType::Volatile, span);
        assert_eq!(loc.extract_text(source), None);
    }

    #[test]
    fn test_extract_text_out_of_bounds_end() {
        let source = "hello";
        let span = Span {
            start: 0,
            end: 10,
            line: 1,
            col: 0,
        };
        let loc = Source::new(SourceType::Volatile, span);
        assert_eq!(loc.extract_text(source), None);
    }

    #[test]
    fn test_extract_text_unicode() {
        let source = "hello 世界";
        let span = Span {
            start: 6,
            end: 12,
            line: 1,
            col: 6,
        };
        let loc = Source::new(SourceType::Volatile, span);
        assert_eq!(loc.extract_text(source), Some("世界".to_string()));
    }

    #[test]
    fn test_text_from() {
        let mut sources = HashMap::new();
        let st = SourceType::Path(Arc::new(PathBuf::from("test.lemma")));
        sources.insert(st.clone(), "hello world".to_string());
        let loc = Source::new(
            st,
            Span {
                start: 0,
                end: 5,
                line: 1,
                col: 0,
            },
        );
        assert_eq!(loc.text_from(&sources).as_deref(), Some("hello"));
    }

    #[test]
    fn from_binding_label_path_and_dependency() {
        assert_eq!(
            SourceType::from_binding_label("app.lemma").unwrap(),
            SourceType::Path(Arc::new(PathBuf::from("app.lemma")))
        );
        assert_eq!(
            SourceType::from_binding_label("@iso/countries").unwrap(),
            SourceType::Dependency("@iso/countries".to_string())
        );
        assert_eq!(
            SourceType::from_binding_label("volatile").unwrap(),
            SourceType::Path(Arc::new(PathBuf::from("volatile")))
        );
        assert!(SourceType::from_binding_label("").is_err());
        assert!(SourceType::from_binding_label("   ").is_err());
    }
}
