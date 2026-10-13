//! JSON API types for Engine boundary documents.
//!
//! Types reachable from [`crate::Engine`] carry no JSON-shaping serde attributes
//! and no custom serde except exact scalar codecs (rationals, date/time strings,
//! decimal strings). JSON shape for `show` / `run` / SDK documents lives only in
//! `lemma::api`.

mod evaluate_data_json;
mod response;
mod show;
mod types;
mod value;

pub use evaluate_data_json::{
    evaluate_data_value_from_json_value, parse_evaluate_data_object, resolve_evaluate_rules,
};
pub use response::{Response, RuleResult};
pub use show::{
    measure_pattern, number_pattern, ratio_pattern, row_def_name, show_document, PathSegment,
    ShowBranch, ShowConversionTarget, ShowExpression, ShowListCommand, ShowRule, SortDirection,
};
pub use types::{
    LemmaType, ListKey, MeasureTrait, MeasureUnit, NamedBound, RatioUnit, RationalFactor,
    TypeDefiningSpec, TypeExtends, TypeSpecification,
};
pub use value::{CalendarResult, LiteralValue, RangeResult, RuleResultValue, ValueKind};
