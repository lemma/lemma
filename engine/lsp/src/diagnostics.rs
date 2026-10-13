use lemma::Error;
use tower_lsp_server::ls_types::{Diagnostic, DiagnosticSeverity, Position, Range};

/// Convert a byte offset to an LSP Position (0-based line and UTF-16 code unit column).
///
/// Walks the text to compute the line and column at the given byte offset.
/// If the offset is beyond the end of the text, returns the position at the end of the text.
pub(crate) fn byte_offset_to_position(text: &str, byte_offset: usize) -> Position {
    let clamped_offset = byte_offset.min(text.len());
    let mut line: u32 = 0;
    let mut line_start_byte: usize = 0;

    for (index, byte) in text.bytes().enumerate() {
        if index == clamped_offset {
            break;
        }
        if byte == b'\n' {
            line += 1;
            line_start_byte = index + 1;
        }
    }

    // If offset points at a newline, treat as start of next line (LSP convention for spans that start at newline).
    let (line, character) =
        if clamped_offset < text.len() && text.as_bytes()[clamped_offset] == b'\n' {
            (line + 1, 0)
        } else {
            let line_slice = &text[line_start_byte..clamped_offset];
            (line, line_slice.encode_utf16().count() as u32)
        };

    Position { line, character }
}

/// Convert an LSP Position (0-based line, UTF-16 column) to a byte offset in `text`.
///
/// Positions past the last line or past the line's last character clamp to the
/// corresponding line end or the end of `text`.
pub(crate) fn position_to_byte_offset(text: &str, position: Position) -> usize {
    let mut current_line = 0u32;
    let mut line_start = 0usize;
    for (index, byte) in text.bytes().enumerate() {
        if current_line == position.line {
            break;
        }
        if byte == b'\n' {
            current_line += 1;
            line_start = index + 1;
        }
    }
    if current_line < position.line {
        return text.len();
    }

    let rest = &text[line_start..];
    let line_end = rest
        .find('\n')
        .map(|i| line_start + i)
        .unwrap_or(text.len());
    let line_slice = &text[line_start..line_end];
    let mut utf16 = 0u32;
    for (byte_index, ch) in line_slice.char_indices() {
        if utf16 >= position.character {
            return line_start + byte_index;
        }
        utf16 += ch.len_utf16() as u32;
    }
    line_end
}

/// True when `position` falls inside `range` using LSP's half-open interval
/// (`range.start <= position < range.end`).
pub(crate) fn position_within_range(position: Position, range: &Range) -> bool {
    let start_before_or_at = position.line > range.start.line
        || (position.line == range.start.line && position.character >= range.start.character);
    let end_after = position.line < range.end.line
        || (position.line == range.end.line && position.character < range.end.character);
    start_before_or_at && end_after
}

/// Convert a Lemma `Span` (byte offsets) to an LSP `Range` using the editor buffer text.
#[must_use]
pub fn span_to_range(text: &str, start_byte: usize, end_byte: usize) -> Range {
    let start_position = byte_offset_to_position(text, start_byte);
    let end_position = byte_offset_to_position(text, end_byte);
    Range {
        start: start_position,
        end: end_position,
    }
}

/// A safe default range anchored at the first character of the spec.
///
/// Used for errors that have no specific source span.
fn default_range() -> Range {
    Range {
        start: Position {
            line: 0,
            character: 0,
        },
        end: Position {
            line: 0,
            character: 0,
        },
    }
}

/// Convert a single Error into an LSP Diagnostic.
///
/// The `text` parameter is the current editor buffer content, used to convert
/// byte offsets to LSP positions.
/// The `file_attribute` is the source identifier for the file being diagnosed,
/// used to filter errors that belong to this file.
pub fn single_error_to_diagnostic(error: &Error, text: &str) -> Diagnostic {
    let range = if let Some(source) = error.location() {
        let start = source.span.start;
        let end = source.span.end;
        if start == 0 && end == 0 {
            default_range()
        } else {
            span_to_range(text, start, end)
        }
    } else {
        default_range()
    };

    let message = format!("{}", error);

    Diagnostic {
        range,
        severity: Some(DiagnosticSeverity::ERROR),
        code: None,
        code_description: None,
        source: Some("lemma".to_string()),
        message,
        related_information: None,
        tags: None,
        data: None,
    }
}

/// Convert all Errors into LSP Diagnostics for a given file.
///
/// - `errors`: the errors to convert.
/// - `text`: the current editor buffer content for the file being diagnosed.
/// - `file_attribute`: the source identifier (filename) for the file being diagnosed.
///   Only errors whose source location `attribute` matches this value are included.
///   Errors without a source location are always included.
pub fn errors_to_diagnostics(
    errors: &[Error],
    text: &str,
    file_attribute: &str,
) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();

    for error in errors {
        let belongs_to_file = match error.location() {
            Some(source) => source.source_type.to_string() == file_attribute,
            None => true,
        };

        if belongs_to_file {
            diagnostics.push(single_error_to_diagnostic(error, text));
        }
    }

    diagnostics
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn byte_offset_to_position_first_line() {
        let text = "hello world";
        let position = byte_offset_to_position(text, 6);
        assert_eq!(position.line, 0);
        assert_eq!(position.character, 6);
    }

    #[test]
    fn byte_offset_to_position_second_line() {
        let text = "hello\nworld";
        let position = byte_offset_to_position(text, 6);
        assert_eq!(position.line, 1);
        assert_eq!(position.character, 0);
    }

    #[test]
    fn byte_offset_to_position_middle_of_second_line() {
        let text = "hello\nworld";
        let position = byte_offset_to_position(text, 9);
        assert_eq!(position.line, 1);
        assert_eq!(position.character, 3);
    }

    #[test]
    fn byte_offset_to_position_at_end_of_text() {
        let text = "hello\nworld";
        let position = byte_offset_to_position(text, 11);
        assert_eq!(position.line, 1);
        assert_eq!(position.character, 5);
    }

    #[test]
    fn byte_offset_to_position_beyond_text_clamps() {
        let text = "hello";
        let position = byte_offset_to_position(text, 100);
        assert_eq!(position.line, 0);
        assert_eq!(position.character, 5);
    }

    #[test]
    fn byte_offset_to_position_empty_text() {
        let text = "";
        let position = byte_offset_to_position(text, 0);
        assert_eq!(position.line, 0);
        assert_eq!(position.character, 0);
    }

    #[test]
    fn byte_offset_to_position_unicode() {
        // "a" (1 byte) + "日" (3 bytes, 1 UTF-16 code unit) + "b" (1 byte)
        let text = "a日b";
        // Offset 1 is start of "日" (3 bytes)
        let position = byte_offset_to_position(text, 1);
        assert_eq!(position.line, 0);
        assert_eq!(position.character, 1); // 'a' = 1 UTF-16 code unit
                                           // Offset 4 is start of "b"
        let position = byte_offset_to_position(text, 4);
        assert_eq!(position.line, 0);
        assert_eq!(position.character, 2); // 'a' + '日' = 2 UTF-16 code units
    }

    #[test]
    fn position_to_byte_offset_round_trips() {
        let text = "hello\nwor日d";
        for offset in 0..=text.len() {
            if !text.is_char_boundary(offset) {
                continue;
            }
            if text.as_bytes().get(offset) == Some(&b'\n') {
                continue;
            }
            let position = byte_offset_to_position(text, offset);
            assert_eq!(
                position_to_byte_offset(text, position),
                offset,
                "offset {offset} position {position:?}"
            );
        }
    }

    #[test]
    fn position_within_range_treats_end_as_exclusive() {
        let range = Range {
            start: Position {
                line: 1,
                character: 5,
            },
            end: Position {
                line: 1,
                character: 10,
            },
        };
        assert!(position_within_range(
            Position {
                line: 1,
                character: 5
            },
            &range
        ));
        assert!(!position_within_range(
            Position {
                line: 1,
                character: 10
            },
            &range
        ));
    }

    #[test]
    fn span_to_range_single_line() {
        let text = "spec test\ndata x: 10";
        let range = span_to_range(text, 9, 20);
        assert_eq!(range.start.line, 1);
        assert_eq!(range.start.character, 0);
        assert_eq!(range.end.line, 1);
        assert_eq!(range.end.character, 10);
    }

    #[test]
    fn span_to_range_multiline() {
        let text = "spec test\ndata x: 10\nrule y: 20";
        let range = span_to_range(text, 9, 32);
        assert_eq!(range.start.line, 1);
        assert_eq!(range.start.character, 0);
        assert_eq!(range.end.line, 2);
        assert_eq!(range.end.character, 10);
    }

    #[test]
    fn errors_to_diagnostics_with_multiple_errors() {
        let error1 =
            Error::resource_limit_exceeded("limit_a", "100", "200", "fix a", None, None, None);
        let error2 =
            Error::resource_limit_exceeded("limit_b", "50", "75", "fix b", None, None, None);
        let diagnostics =
            errors_to_diagnostics(&[error1, error2], "spec test\ndata x: 10", "test.lemma");
        assert_eq!(diagnostics.len(), 2);
    }

    #[test]
    fn errors_to_diagnostics_filters_by_file_attribute() {
        use lemma::Span;

        let error_in_file = Error::parsing(
            "bad syntax",
            lemma::Source::new(
                lemma::SourceType::Path(std::sync::Arc::new(std::path::PathBuf::from(
                    "file_a.lemma",
                ))),
                Span {
                    start: 0,
                    end: 8,
                    line: 1,
                    col: 1,
                },
            ),
            None::<String>,
        );
        let error_in_other_file = Error::parsing(
            "also bad",
            lemma::Source::new(
                lemma::SourceType::Path(std::sync::Arc::new(std::path::PathBuf::from(
                    "file_b.lemma",
                ))),
                Span {
                    start: 0,
                    end: 5,
                    line: 1,
                    col: 1,
                },
            ),
            None::<String>,
        );

        let text = "spec test";
        let diagnostics =
            errors_to_diagnostics(&[error_in_file, error_in_other_file], text, "file_a.lemma");
        assert_eq!(diagnostics.len(), 1);
        assert!(diagnostics[0].message.contains("bad syntax"));
    }

    #[test]
    fn errors_keep_error_severity() {
        let error =
            Error::resource_limit_exceeded("limit_a", "100", "200", "fix a", None, None, None);
        let diagnostics = errors_to_diagnostics(&[error], "spec test", "test.lemma");
        assert_eq!(diagnostics.len(), 1);
        assert_eq!(diagnostics[0].severity, Some(DiagnosticSeverity::ERROR));
    }
}
