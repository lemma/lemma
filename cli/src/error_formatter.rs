use ariadne::{Color, Label, Report, ReportKind, Source};
use lemma::Error;
use lemma::ErrorDetails;
use std::collections::HashMap;

fn format_details(
    error_type: &str,
    details: &ErrorDetails,
    label_message: &str,
    sources: &HashMap<lemma::SourceType, String>,
) -> String {
    let Some(ref src) = details.source else {
        return format!("{}: {}", error_type, details.message);
    };

    let source_type_str = src.source_type.to_string();

    let Some(full_content) = sources.get(&src.source_type) else {
        return format!(
            "{}: {} ({}:{})",
            error_type, details.message, source_type_str, src.span.line
        );
    };

    let mut output = Vec::new();

    let header = match details.spec_context_name.as_ref() {
        Some(name) => format!(
            "{}: {} (in spec '{}', file {}:{})",
            error_type, details.message, name, source_type_str, src.span.line
        ),
        None => format!(
            "{}: {} ({}:{})",
            error_type, details.message, source_type_str, src.span.line
        ),
    };

    let span = (source_type_str.as_str(), src.span.start..src.span.end);
    let mut report = Report::build(ReportKind::Error, span.clone())
        .with_message(header)
        .with_label(
            Label::new(span)
                .with_message(label_message)
                .with_color(Color::Red),
        );

    if let Some(suggestion) = &details.suggestion {
        report = report.with_help(suggestion);
    }

    let content: &str = full_content.as_str();
    if report
        .finish()
        .write(
            (source_type_str.as_str(), Source::from(content)),
            &mut output,
        )
        .is_err()
    {
        return format!(
            "{}: {} ({}:{})",
            error_type, details.message, source_type_str, src.span.line
        );
    }
    String::from_utf8_lossy(&output).to_string()
}

#[must_use]
pub fn format_error(error: &Error, sources: &HashMap<lemma::SourceType, String>) -> String {
    let fmt = |typ: &str, details: &ErrorDetails, label: &str| {
        format_details(typ, details, label, sources)
    };
    match error {
        Error::Parsing(details) => fmt("Parse error", details, ""),
        Error::Validation(details) => fmt("Validation error", details, ""),
        Error::Registry {
            details,
            identifier,
            kind,
        } => fmt(&format!("Registry error ({})", kind), details, identifier),
        Error::MissingRepository {
            details,
            repository,
        } => fmt("Missing repository", details, repository),
        Error::ResourceLimitExceeded {
            details,
            limit_name,
            limit_value,
            actual_value,
        } => fmt(
            &format!("Resource limit exceeded: {limit_name} (limit: {limit_value}, actual: {actual_value})"),
            details,
            "",
        ),
        Error::Request { details, .. } => fmt("Request error", details, ""),
    }
}
