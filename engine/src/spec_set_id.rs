//! Parse spec set identifiers: a single spec set `name`.
//!
//! Effective datetime is never embedded in the id; pass it separately
//! (e.g. CLI `--effective`, HTTP `Accept-Datetime`).

use crate::error::Error;
use crate::limits::MAX_SPEC_NAME_LENGTH;

/// Validate and normalize a spec set identifier (a spec name in the default repository).
///
/// Trims whitespace and rejects empty input, embedded whitespace, and the
/// version sigils `~` / `^` (which once denoted revisions and are now reserved).
/// Evaluation entry points use the default repository, so temporal or version
/// sigils on the spec set name are not accepted here.
pub fn parse_spec_set_id(s: &str) -> Result<String, Error> {
    let s = s.trim();
    if s.is_empty() {
        return Err(Error::request(
            "Spec set identifier cannot be empty",
            Some("Use a spec set name"),
        ));
    }

    if s.contains('~') {
        return Err(Error::request(
            "Spec set identifier cannot contain '~'",
            Some("Use a plain spec name"),
        ));
    }

    if s.contains('^') {
        return Err(Error::request(
            "Spec set identifier cannot contain '^'",
            Some("Use a plain spec name"),
        ));
    }

    if s.split_whitespace().count() != 1 {
        return Err(Error::request(
            "Spec set identifier must be a single spec name",
            Some("Specs run from the default repository; do not include an extra repository qualifier"),
        ));
    }

    if s.len() > MAX_SPEC_NAME_LENGTH {
        return Err(Error::request(
            format!(
                "Spec name exceeds maximum length ({} characters)",
                MAX_SPEC_NAME_LENGTH
            ),
            Some("Shorten the spec name"),
        ));
    }

    Ok(s.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_name_only() {
        assert_eq!(parse_spec_set_id("pricing").unwrap(), "pricing".to_string());
        assert_eq!(
            parse_spec_set_id("  pricing  ").unwrap(),
            "pricing".to_string()
        );
    }

    #[test]
    fn repository_qualifier_rejected() {
        assert!(parse_spec_set_id("nl/tax pricing").is_err());
        assert!(parse_spec_set_id("@iso/countries alpha2").is_err());
    }

    #[test]
    fn tilde_rejected() {
        assert!(parse_spec_set_id("pricing~a1b2c3d4").is_err());
    }

    #[test]
    fn caret_rejected() {
        assert!(parse_spec_set_id("pricing^a1b2c3d4").is_err());
    }

    #[test]
    fn empty_err() {
        assert!(parse_spec_set_id("").is_err());
        assert!(parse_spec_set_id("   ").is_err());
    }
}
