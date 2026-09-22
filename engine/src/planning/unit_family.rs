//! Plan-time catalog of measure/ratio family units for rule results and Show rule schemas.
//!
//! Built once from the finalized expression-scope [`UnitIndex`]. Eval and show read the
//! catalog instead of re-walking the index.

use crate::literals::{MeasureUnits, RatioUnits};
use crate::planning::semantics::{
    range_element_type_specification, LemmaType, TypeExtends, TypeSpecification,
};
use crate::planning::unit_index::UnitIndex;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, HashMap};
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
    by_family: BTreeMap<String, FamilyUnitEntry>,
}

impl FamilyUnitCatalog {
    #[must_use]
    pub(crate) fn build(unit_index: &UnitIndex) -> Self {
        let mut families: BTreeSet<String> = BTreeSet::new();
        for lemma_type in unit_index.values() {
            if let Some(family) = lemma_type.unit_family_name() {
                families.insert(family.to_string());
            }
        }

        let mut by_family = BTreeMap::new();
        for family in families {
            by_family.insert(family.clone(), build_family_entry(unit_index, &family));
        }
        Self { by_family }
    }

    /// Family entry for `lemma_type`, resolving measure/ratio range wrappers to their element family.
    #[must_use]
    pub(crate) fn entry_for_type(&self, lemma_type: &LemmaType) -> Option<&FamilyUnitEntry> {
        if let Some(family) = unit_expansion_family_name(lemma_type) {
            return self.by_family.get(&family);
        }
        self.entry_for_anonymous_declared_units(lemma_type)
    }

    /// When the type has no family name (anonymous measure/range) but declares units,
    /// use the unique catalog family that contains every declared bare unit.
    fn entry_for_anonymous_declared_units(
        &self,
        lemma_type: &LemmaType,
    ) -> Option<&FamilyUnitEntry> {
        let declared = lemma_type
            .measure_unit_names()
            .or_else(|| lemma_type.ratio_unit_names())?;
        if declared.is_empty() {
            return None;
        }
        let mut matched_family: Option<&str> = None;
        for (family, entry) in &self.by_family {
            let contains_all = declared.iter().all(|name| {
                entry
                    .family_bare_names
                    .iter()
                    .any(|bare| bare.as_str() == *name)
            });
            if !contains_all {
                continue;
            }
            if matched_family.is_some() {
                return None;
            }
            matched_family = Some(family.as_str());
        }
        matched_family.and_then(|family| self.by_family.get(family))
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

fn build_family_entry(unit_index: &UnitIndex, family: &str) -> FamilyUnitEntry {
    let family_types: Vec<&Arc<LemmaType>> = unit_index
        .values()
        .filter(|candidate| candidate.unit_family_name() == Some(family))
        .collect();

    let mut type_by_name: HashMap<String, &Arc<LemmaType>> = HashMap::new();
    for lemma_type in &family_types {
        let name = lemma_type.name().to_string();
        match type_by_name.entry(name.clone()) {
            std::collections::hash_map::Entry::Vacant(slot) => {
                slot.insert(*lemma_type);
            }
            std::collections::hash_map::Entry::Occupied(slot) => {
                if !std::sync::Arc::ptr_eq(slot.get(), lemma_type) {
                    panic!(
                        "BUG: duplicate type name '{name}' in family '{family}' maps to distinct types"
                    );
                }
            }
        }
    }

    let merged_measure_units = merged_measure_units_for_family(unit_index, family, &type_by_name);
    let merged_ratio_units = merged_ratio_units_for_family(unit_index, family, &type_by_name);

    let family_bare_names: Arc<[String]> = merged_measure_units
        .as_ref()
        .map(|units| {
            units
                .iter()
                .map(|unit| unit.name.clone())
                .collect::<Vec<_>>()
                .into()
        })
        .or_else(|| {
            merged_ratio_units.as_ref().map(|units| {
                units
                    .iter()
                    .map(|unit| unit.name.clone())
                    .collect::<Vec<_>>()
                    .into()
            })
        })
        .unwrap_or_else(|| Arc::from([]));

    FamilyUnitEntry {
        family_bare_names,
        merged_measure_units,
        merged_ratio_units,
    }
}

fn merged_measure_units_for_family(
    unit_index: &UnitIndex,
    family: &str,
    type_by_name: &HashMap<String, &Arc<LemmaType>>,
) -> Option<MeasureUnits> {
    let mut types: Vec<&Arc<LemmaType>> = unit_index
        .values()
        .filter(|candidate| candidate.measure_family_name() == Some(family))
        .collect();
    types.sort_by(|left, right| {
        type_extension_depth(left.as_ref(), type_by_name)
            .cmp(&type_extension_depth(right.as_ref(), type_by_name))
            .then_with(|| left.name().cmp(&right.name()))
    });

    let mut merged = MeasureUnits::new();
    let mut seen = BTreeSet::new();
    for lemma_type in types {
        let (TypeSpecification::Measure { units, .. }
        | TypeSpecification::MeasureRange { units, .. }) = &lemma_type.specifications
        else {
            continue;
        };
        for unit in units.iter() {
            if seen.insert(unit.name.clone()) {
                merged.push(unit.clone());
            }
        }
    }
    if merged.is_empty() {
        None
    } else {
        Some(merged)
    }
}

fn merged_ratio_units_for_family(
    unit_index: &UnitIndex,
    family: &str,
    type_by_name: &HashMap<String, &Arc<LemmaType>>,
) -> Option<RatioUnits> {
    let mut types: Vec<&Arc<LemmaType>> = unit_index
        .values()
        .filter(|candidate| candidate.ratio_family_name() == Some(family))
        .collect();
    types.sort_by(|left, right| {
        type_extension_depth(left.as_ref(), type_by_name)
            .cmp(&type_extension_depth(right.as_ref(), type_by_name))
            .then_with(|| left.name().cmp(&right.name()))
    });

    let mut merged = RatioUnits::new();
    let mut seen = BTreeSet::new();
    for lemma_type in types {
        let (TypeSpecification::Ratio { units, .. } | TypeSpecification::RatioRange { units, .. }) =
            &lemma_type.specifications
        else {
            continue;
        };
        for unit in units.iter() {
            if seen.insert(unit.name.clone()) {
                merged.push(unit.clone());
            }
        }
    }
    if merged.is_empty() {
        None
    } else {
        Some(merged)
    }
}

fn unit_expansion_family_name(lemma_type: &LemmaType) -> Option<String> {
    if let Some(family) = lemma_type.unit_family_name() {
        return Some(family.to_string());
    }
    let element = range_element_type_specification(&lemma_type.specifications)?;
    LemmaType::primitive(element.clone())
        .unit_family_name()
        .map(str::to_string)
}

fn type_extension_depth(
    lemma_type: &LemmaType,
    family_types: &HashMap<String, &Arc<LemmaType>>,
) -> usize {
    let mut depth = 0usize;
    let mut current = lemma_type;
    loop {
        match &current.extends {
            TypeExtends::Primitive => return depth,
            TypeExtends::Custom { parent, .. } => {
                depth += 1;
                let Some(parent_type) = family_types.get(parent.as_str()) else {
                    return depth;
                };
                current = parent_type.as_ref();
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
        let money = Arc::new(LemmaType::new(
            "money".into(),
            TypeSpecification::Measure {
                units: money_units,
                decimals: None,
                traits: vec![],
                decomposition: None,
                minimum: None,
                maximum: None,
                help: String::new(),
            },
            TypeExtends::Primitive,
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
                help: String::new(),
            },
            TypeExtends::custom_local("money".to_string(), "money".to_string()),
        ));

        let mut index = UnitIndex::new();
        index
            .merge_measure_unit("eur".into(), &money, "money", None, "money")
            .expect("eur");
        index
            .merge_measure_unit("usd".into(), &money, "money", None, "money")
            .expect("usd");
        index
            .merge_measure_unit("gbp".into(), &price, "price", None, "money")
            .expect("gbp");

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
                    help: String::new(),
                },
                TypeExtends::Primitive,
            ))
        };
        let placeholder = Arc::new(LemmaType::new(
            "unused".into(),
            TypeSpecification::measure(),
            TypeExtends::Primitive,
        ));
        let mut index = UnitIndex::new();
        index
            .merge_ratio_unit(
                "percent".into(),
                &ratio,
                "rate",
                Some("alpha".into()),
                &placeholder,
            )
            .expect("alpha alias");
        index
            .merge_ratio_unit(
                "percent".into(),
                &ratio,
                "rate",
                Some("beta".into()),
                &placeholder,
            )
            .expect("beta alias");
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
