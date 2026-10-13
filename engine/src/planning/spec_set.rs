//! Source-level grouping: specs sharing a name, keyed by effective_from.

use crate::parsing::ast::{DateTimeValue, EffectiveDate, LemmaRepository, LemmaSpec};
use std::collections::BTreeMap;
use std::sync::Arc;

/// All spec versions sharing a (repository, name) identity, keyed by effective_from.
///
/// The owning [`LemmaRepository`] is held by `Arc` so the set carries repository identity as
/// a real memory reference instead of relying on string parsing.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct LemmaSpecSet {
    #[serde(with = "crate::snapshot::shared::arc")]
    pub repository: Arc<LemmaRepository>,
    pub name: String,
    specs: BTreeMap<EffectiveDate, LemmaSpec>,
}

impl LemmaSpecSet {
    #[must_use]
    pub fn new(repository: Arc<LemmaRepository>, name: String) -> Self {
        Self {
            repository,
            name,
            specs: BTreeMap::new(),
        }
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.specs.is_empty()
    }

    /// Exact identity by `effective_from` key.
    #[must_use]
    pub fn get_exact(&self, effective_from: Option<&DateTimeValue>) -> Option<&LemmaSpec> {
        let key = EffectiveDate::from_option(effective_from.cloned());
        self.specs.get(&key)
    }

    /// Insert a spec. Returns `false` if the same `effective_from` already exists.
    pub fn insert(&mut self, spec: LemmaSpec) -> bool {
        assert_eq!(
            spec.name, self.name,
            "BUG: spec name mismatch in LemmaSpecSet::insert"
        );
        let key = spec.effective_from.clone();
        if self.specs.contains_key(&key) {
            return false;
        }
        self.specs.insert(key, spec);
        true
    }

    /// Remove by `effective_from` key. Returns whether a row was removed.
    pub fn remove(&mut self, effective_from: Option<&DateTimeValue>) -> bool {
        let key = EffectiveDate::from_option(effective_from.cloned());
        self.specs.remove(&key).is_some()
    }

    pub fn iter_specs(&self) -> impl Iterator<Item = &LemmaSpec> + '_ {
        self.specs.values()
    }

    /// Every spec paired with its half-open `[effective_from, effective_to)` range.
    ///
    /// - `effective_from = None` on the first row means no earlier version exists.
    /// - `effective_to = None` on the last row means no successor (this is the
    ///   latest loaded version; its validity is unbounded forward).
    /// - Otherwise `effective_to` equals the next row's `effective_from`
    ///   (exclusive end of this row's validity).
    ///
    /// Iteration order matches [`Self::iter_specs`] (ascending by `effective_from`).
    pub fn iter_with_ranges(
        &self,
    ) -> impl Iterator<Item = (&LemmaSpec, Option<DateTimeValue>, Option<DateTimeValue>)> + '_ {
        self.iter_specs().map(move |spec| {
            let (effective_from, effective_to) = self.effective_range(spec);
            (spec, effective_from, effective_to)
        })
    }

    /// Spec active at `effective`. Each spec covers `[effective_from, next.effective_from)`.
    /// The last spec covers `[effective_from, +∞)`.
    #[must_use]
    pub fn spec_at(&self, effective: &EffectiveDate) -> Option<&LemmaSpec> {
        self.specs
            .range(..=effective.clone())
            .next_back()
            .map(|(_, spec)| spec)
    }

    /// Returns the effective range `[from, to)` for a spec in this set.
    ///
    /// - `from`: `spec.effective_from()` (None = -∞)
    /// - `to`: next temporal version's `effective_from`, or None (+∞) if no successor.
    pub fn effective_range(
        &self,
        spec: &LemmaSpec,
    ) -> (Option<DateTimeValue>, Option<DateTimeValue>) {
        let from = spec.effective_from().cloned();
        let key = spec.effective_from.clone();
        let exact = self.specs.get_key_value(&key).unwrap_or_else(|| {
            unreachable!(
                "BUG: effective_range called with spec '{}' not in spec set",
                spec.name
            )
        });
        let to = self
            .specs
            .range((
                std::ops::Bound::Excluded(exact.0),
                std::ops::Bound::Unbounded,
            ))
            .next()
            .and_then(|(_, next)| next.effective_from().cloned());
        (from, to)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parsing::ast::LemmaSpec;

    fn main_repository() -> Arc<LemmaRepository> {
        Arc::new(LemmaRepository::new(None))
    }

    use crate::literals::DateGranularity;

    fn date(year: i32, month: u32, day: u32) -> DateTimeValue {
        DateTimeValue {
            year,
            month,
            day,
            hour: 0,
            minute: 0,
            second: 0,
            microsecond: 0,
            timezone: None,
            granularity: DateGranularity::Full,
        }
    }

    fn make_spec(name: &str) -> LemmaSpec {
        LemmaSpec::new(name.to_string())
    }

    fn make_spec_with_range(name: &str, effective_from: Option<DateTimeValue>) -> LemmaSpec {
        let mut spec = LemmaSpec::new(name.to_string());
        spec.effective_from = EffectiveDate::from_option(effective_from);
        spec
    }

    /// A `LemmaSpecSet` carries `(repository, name)` identity; inserting a spec
    /// whose name differs corrupts that identity silently (`spec_at`/`get_exact`
    /// would return a spec that does not belong to this set). This is an
    /// invariant violation and must crash, not be accepted.
    #[test]
    #[should_panic(expected = "BUG")]
    fn insert_panics_on_spec_name_mismatch() {
        let mut ss = LemmaSpecSet::new(main_repository(), "a".to_string());
        ss.insert(make_spec("b"));
    }

    #[test]
    fn effective_range_unbounded_single_spec() {
        let mut ss = LemmaSpecSet::new(main_repository(), "a".to_string());
        let spec = make_spec("a");
        assert!(ss.insert(spec));
        let stored = ss.get_exact(None).expect("inserted");

        let (from, to) = ss.effective_range(stored);
        assert_eq!(from, None);
        assert_eq!(to, None);
    }

    #[test]
    fn effective_range_soft_end_from_next_spec() {
        let mut ss = LemmaSpecSet::new(main_repository(), "a".to_string());
        assert!(ss.insert(make_spec_with_range("a", Some(date(2025, 1, 1)))));
        assert!(ss.insert(make_spec_with_range("a", Some(date(2025, 6, 1)))));

        let v1 = ss.get_exact(Some(&date(2025, 1, 1))).expect("v1");
        let (from, to) = ss.effective_range(v1);
        assert_eq!(from, Some(date(2025, 1, 1)));
        assert_eq!(to, Some(date(2025, 6, 1)));

        let v2 = ss.get_exact(Some(&date(2025, 6, 1))).expect("v2");
        let (from, to) = ss.effective_range(v2);
        assert_eq!(from, Some(date(2025, 6, 1)));
        assert_eq!(to, None);
    }

    /// `iter_with_ranges` yields each spec paired with its half-open
    /// `[effective_from, effective_to)` range. Earlier rows end where the
    /// next row begins; the latest row's `effective_to` is `None`.
    #[test]
    fn iter_with_ranges_yields_specs_paired_with_half_open_range() {
        let mut ss = LemmaSpecSet::new(main_repository(), "a".to_string());
        assert!(ss.insert(make_spec_with_range("a", Some(date(2025, 1, 1)))));
        assert!(ss.insert(make_spec_with_range("a", Some(date(2025, 6, 1)))));

        let entries: Vec<_> = ss.iter_with_ranges().collect();
        assert_eq!(entries.len(), 2);

        let (spec_0, from_0, to_0) = &entries[0];
        assert_eq!(spec_0.effective_from(), Some(&date(2025, 1, 1)));
        assert_eq!(from_0, &Some(date(2025, 1, 1)));
        assert_eq!(
            to_0,
            &Some(date(2025, 6, 1)),
            "earlier row ends at the next row's effective_from"
        );

        let (spec_1, from_1, to_1) = &entries[1];
        assert_eq!(spec_1.effective_from(), Some(&date(2025, 6, 1)));
        assert_eq!(from_1, &Some(date(2025, 6, 1)));
        assert_eq!(
            to_1, &None,
            "latest row has no successor; effective_to is None"
        );
    }

    #[test]
    fn effective_range_unbounded_start_with_successor() {
        let mut ss = LemmaSpecSet::new(main_repository(), "a".to_string());
        assert!(ss.insert(make_spec("a")));
        assert!(ss.insert(make_spec_with_range("a", Some(date(2025, 3, 1)))));

        let v1 = ss.get_exact(None).expect("v1");
        let (from, to) = ss.effective_range(v1);
        assert_eq!(from, None);
        assert_eq!(to, Some(date(2025, 3, 1)));
    }
}
