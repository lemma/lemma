//! Plan-time catalog of measure/ratio family units for rule results and Show rule schemas.
//!
//! Built once from the finalized expression-scope [`UnitIndex`]. Eval and show read the
//! catalog instead of re-walking the index.

use crate::literals::{MeasureUnits, RatioUnits};
use crate::planning::semantics::{LemmaType, TypeFamily, TypeSpecification};
use crate::planning::unit_index::UnitIndex;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

/// Precomputed units for one measure or ratio family in expression scope.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct FamilyUnitEntry {
    pub family_bare_names: Arc<[String]>,
    pub merged_measure_units: Option<MeasureUnits>,
    pub merged_ratio_units: Option<RatioUnits>,
}

/// Family-keyed unit expansion data for a plan slice.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct FamilyUnitCatalog {
    by_family: BTreeMap<TypeFamily, FamilyUnitEntry>,
}

impl FamilyUnitCatalog {
    /// Units per family in unit-index order. Types resolve parent-first, so a family's
    /// inherited units precede the units its extensions add.
    #[must_use]
    pub(crate) fn build(unit_index: &UnitIndex) -> Self {
        let mut measure: BTreeMap<TypeFamily, MeasureUnits> = BTreeMap::new();
        let mut ratio: BTreeMap<TypeFamily, RatioUnits> = BTreeMap::new();
        let mut seen: BTreeSet<(TypeFamily, &str)> = BTreeSet::new();
        for (bare, owner) in unit_index.iter_owners() {
            let lemma_type = owner.lemma_type();
            let Some(family) = lemma_type.unit_family() else {
                continue;
            };
            if !seen.insert((family.clone(), bare)) {
                continue;
            }
            match &lemma_type.specifications {
                TypeSpecification::Measure { units, .. } => {
                    let unit = units
                        .get(bare)
                        .expect("BUG: unit index owner must declare its bare unit");
                    measure
                        .entry(family.clone())
                        .or_default()
                        .push(unit.clone());
                }
                TypeSpecification::Ratio { units, .. } => {
                    let unit = units
                        .get(bare)
                        .expect("BUG: unit index owner must declare its bare unit");
                    ratio.entry(family.clone()).or_default().push(unit.clone());
                }
                _ => unreachable!("BUG: unit family owner must be a measure or ratio type"),
            }
        }
        for (_, owner) in unit_index.iter_owners() {
            let lemma_type = owner.lemma_type();
            let Some(family) = lemma_type.unit_family() else {
                continue;
            };
            match &lemma_type.specifications {
                TypeSpecification::Measure { units, .. } => {
                    for unit in units.iter() {
                        if seen.insert((family.clone(), unit.name.as_str())) {
                            measure
                                .entry(family.clone())
                                .or_default()
                                .push(unit.clone());
                        }
                    }
                }
                TypeSpecification::Ratio { units, .. } => {
                    for unit in units.iter() {
                        if seen.insert((family.clone(), unit.name.as_str())) {
                            ratio.entry(family.clone()).or_default().push(unit.clone());
                        }
                    }
                }
                _ => unreachable!("BUG: unit family owner must be a measure or ratio type"),
            }
        }

        let mut by_family = BTreeMap::new();
        for (family, units) in measure {
            let family_bare_names = units.iter().map(|unit| unit.name.clone()).collect();
            by_family.insert(
                family,
                FamilyUnitEntry {
                    family_bare_names,
                    merged_measure_units: Some(units),
                    merged_ratio_units: None,
                },
            );
        }
        for (family, units) in ratio {
            let family_bare_names = units.iter().map(|unit| unit.name.clone()).collect();
            let previous = by_family.insert(
                family,
                FamilyUnitEntry {
                    family_bare_names,
                    merged_measure_units: None,
                    merged_ratio_units: Some(units),
                },
            );
            assert!(
                previous.is_none(),
                "BUG: a type family is either measure or ratio"
            );
        }
        Self { by_family }
    }

    /// Family entry for `lemma_type`; measure and ratio ranges share their element's family.
    #[must_use]
    pub(crate) fn entry_for_type(&self, lemma_type: &LemmaType) -> Option<&FamilyUnitEntry> {
        match &lemma_type.specifications {
            TypeSpecification::Measure { .. }
            | TypeSpecification::MeasureRange { .. }
            | TypeSpecification::Ratio { .. }
            | TypeSpecification::RatioRange { .. } => self.by_family.get(lemma_type.family()?),
            _ => None,
        }
    }

    /// Ordered bare unit names for rule-result maps: anchor declared units first, then family bares.
    ///
    /// Returns a shared [`Arc`] when the type's declared units are empty or identical to the
    /// family's precomputed list (the common percent/money paths).
    #[must_use]
    pub(crate) fn ordered_bare_names_for_type(&self, lemma_type: &LemmaType) -> Arc<[String]> {
        let Some(entry) = self.entry_for_type(lemma_type) else {
            return Arc::from(declared_bare_names_only(lemma_type));
        };
        let mut names = Vec::new();
        let mut seen = BTreeSet::new();
        append_declared_unit_names(lemma_type, &mut names, &mut seen);
        if names.is_empty() {
            return Arc::clone(&entry.family_bare_names);
        }
        if names.as_slice() == entry.family_bare_names.as_ref() {
            return Arc::clone(&entry.family_bare_names);
        }
        for bare in entry.family_bare_names.iter() {
            if seen.insert(bare.clone()) {
                names.push(bare.clone());
            }
        }
        Arc::from(names)
    }

    /// `rule_type` with family-merged unit metadata for Show rule schemas.
    #[must_use]
    pub(crate) fn rule_type_for_show(&self, rule_type: &LemmaType) -> LemmaType {
        let Some(entry) = self.entry_for_type(rule_type) else {
            return rule_type.clone();
        };
        let mut out = rule_type.clone();
        match &mut out.specifications {
            TypeSpecification::Measure { units, .. } => {
                if let Some(merged) = &entry.merged_measure_units {
                    *units = merged.clone();
                }
            }
            TypeSpecification::MeasureRange { units, .. } => {
                if let Some(merged) = &entry.merged_measure_units {
                    *units = merged.clone();
                }
            }
            TypeSpecification::Ratio { units, .. } => {
                if let Some(merged) = &entry.merged_ratio_units {
                    *units = merged.clone();
                }
            }
            TypeSpecification::RatioRange { units, .. } => {
                if let Some(merged) = &entry.merged_ratio_units {
                    *units = merged.clone();
                }
            }
            _ => {}
        }
        out
    }
}

/// Bare unit names declared on `lemma_type` only (Show data fill/suggestion).
#[must_use]
pub(crate) fn declared_bare_names_only(lemma_type: &LemmaType) -> Vec<String> {
    let mut names = Vec::new();
    let mut seen = BTreeSet::new();
    append_declared_unit_names(lemma_type, &mut names, &mut seen);
    names
}

pub(crate) fn append_declared_unit_names(
    lemma_type: &LemmaType,
    names: &mut Vec<String>,
    seen: &mut BTreeSet<String>,
) {
    if let Some(measure_names) = lemma_type.measure_unit_names() {
        for unit_name in measure_names {
            if seen.insert(unit_name.to_string()) {
                names.push(unit_name.to_string());
            }
        }
    } else if let Some(ratio_names) = lemma_type.ratio_unit_names() {
        for unit_name in ratio_names {
            if seen.insert(unit_name.to_string()) {
                names.push(unit_name.to_string());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::computation::rational::rational_one;
    use crate::literals::{MeasureUnit, MeasureUnits};
    use crate::planning::semantics::{LemmaType, TypeExtends, TypeSpecification};
    use crate::planning::unit_index::UnitOwner;

    #[test]
    fn family_unit_names_union_across_extensions() {
        let mut money_units = MeasureUnits::new();
        money_units.push(MeasureUnit {
            name: "eur".into(),
            factor: rational_one(),
            derived_measure_factors: Vec::new(),
            decomposition: Default::default(),
            minimum: None,
            maximum: None,
            suggestion_magnitude: None,
        });
        money_units.push(MeasureUnit {
            name: "usd".into(),
            factor: rational_one(),
            derived_measure_factors: Vec::new(),
            decomposition: Default::default(),
            minimum: None,
            maximum: None,
            suggestion_magnitude: None,
        });
        let family = crate::planning::semantics::tests::test_type_family("money");
        let money = Arc::new(LemmaType::new(
            "money".into(),
            TypeSpecification::Measure {
                units: money_units,
                decimals: None,
                traits: vec![],
                decomposition: None,
                minimum: None,
                maximum: None,
                unit: None,
                help: String::new(),
            },
            TypeExtends::custom_local("measure".to_string(), family.clone()),
        ));
        let price = Arc::new(LemmaType::new(
            "price".into(),
            TypeSpecification::Measure {
                units: {
                    let mut units = MeasureUnits::new();
                    units.push(MeasureUnit {
                        name: "gbp".into(),
                        factor: rational_one(),
                        derived_measure_factors: Vec::new(),
                        decomposition: Default::default(),
                        minimum: None,
                        maximum: None,
                        suggestion_magnitude: None,
                    });
                    units
                },
                decimals: None,
                traits: vec![],
                decomposition: None,
                minimum: None,
                maximum: None,
                unit: None,
                help: String::new(),
            },
            TypeExtends::custom_local("money".to_string(), family),
        ));

        let mut index = UnitIndex::new();
        index.insert_owner(
            "eur".into(),
            UnitOwner::Named {
                import_path: Vec::new(),
                type_name: "money".into(),
                lemma_type: Arc::clone(&money),
            },
        );
        index.insert_owner(
            "usd".into(),
            UnitOwner::Named {
                import_path: Vec::new(),
                type_name: "money".into(),
                lemma_type: Arc::clone(&money),
            },
        );
        index.insert_owner(
            "gbp".into(),
            UnitOwner::Named {
                import_path: Vec::new(),
                type_name: "price".into(),
                lemma_type: Arc::clone(&price),
            },
        );

        let catalog = FamilyUnitCatalog::build(&index);
        let names = catalog.ordered_bare_names_for_type(money.as_ref());
        assert_eq!(
            names.as_ref(),
            &["eur".to_string(), "usd".to_string(), "gbp".to_string()][..]
        );
        let show_type = catalog.rule_type_for_show(money.as_ref());
        let show_units: Vec<&str> = show_type
            .measure_unit_names()
            .expect("measure units")
            .into_iter()
            .collect();
        assert_eq!(show_units, vec!["eur", "usd", "gbp"]);
    }

    #[test]
    fn family_catalog_includes_percent_with_dual_import_owners() {
        use crate::literals::{RatioUnit, RatioUnits};
        use crate::planning::semantics::TypeSpecification;

        let ratio = {
            let units = RatioUnits(vec![RatioUnit {
                name: "percent".into(),
                value: rational_one(),
                minimum: None,
                maximum: None,
                suggestion_magnitude: None,
            }]);
            Arc::new(LemmaType::new(
                "rate".into(),
                TypeSpecification::Ratio {
                    minimum: None,
                    maximum: None,
                    decimals: None,
                    units,
                    unit: None,
                    help: String::new(),
                },
                TypeExtends::custom_local(
                    "ratio".to_string(),
                    crate::planning::semantics::tests::test_type_family("rate"),
                ),
            ))
        };
        let mut index = UnitIndex::new();
        index.insert_owner(
            "percent".into(),
            UnitOwner::Named {
                import_path: vec!["alpha".into()],
                type_name: "rate".into(),
                lemma_type: Arc::clone(&ratio),
            },
        );
        index.insert_owner(
            "percent".into(),
            UnitOwner::Named {
                import_path: vec!["beta".into()],
                type_name: "rate".into(),
                lemma_type: Arc::clone(&ratio),
            },
        );
        assert!(
            index.unique_owner("percent").is_none(),
            "dual import aliases must leave percent without a unique bare owner"
        );

        let catalog = FamilyUnitCatalog::build(&index);
        let entry = catalog
            .entry_for_type(ratio.as_ref())
            .expect("ratio family entry");
        let merged = entry
            .merged_ratio_units
            .as_ref()
            .expect("merged ratio units");
        assert!(merged.get("percent").is_ok());
        assert_eq!(
            entry.family_bare_names.as_ref(),
            &["percent".to_string()][..]
        );
    }
}
