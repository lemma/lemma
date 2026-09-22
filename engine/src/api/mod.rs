//! JSON API types for Engine boundary documents.
//!
//! Types reachable from [`crate::Engine`] carry no JSON-shaping serde attributes
//! and no custom serde except exact scalar codecs (rationals, date/time strings,
//! decimal strings). JSON shape for `show` / `run` / SDK documents lives only in
//! `lemma::api`.

mod response;
mod show;
mod types;
mod value;

pub use response::{Response, RuleResult};
pub use show::{
    PathSegment, Show, ShowBranch, ShowConversionTarget, ShowData, ShowExpression, ShowRule,
    ShowVersion,
};
pub use types::{
    LemmaType, MeasureTrait, MeasureUnit, NamedBound, RatioUnit, RationalFactor, TypeDefiningSpec,
    TypeExtends, TypeSpecification,
};
pub use value::{CalendarResult, LiteralValue, RangeResult, RuleResultValue, ValueKind};
