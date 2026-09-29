//! Expression-scope unit index: one declarer per bare unit name.
//! Qualify with Type.unit or alias.Type.unit (not alias.unit).

use crate::planning::semantics::{LemmaType, TypeSpecification};
use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::sync::Arc;

/// One measure/ratio type that declares a given bare unit name.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UnitOwner {
    pub owning_type: Arc<LemmaType>,
    /// Local typedef name or imported typedef name.
    pub type_name: String,
    /// `None` = local to the consumer spec; `Some(alias)` = contributed by `uses alias`.
    pub import_alias: Option<String>,
}

/// Conflict while merging a unit owner into the index during type resolution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UnitMergeConflict {
    Ambiguous {
        unit: String,
        existing_name: String,
        new_name: String,
    },
    ConflictingFactors {
        unit: String,
        family: String,
    },
    AmbiguousRatio {
        unit: String,
        existing_name: String,
        new_name: String,
    },
}

/// Expression-scope unit names → owning measure/ratio types.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct UnitIndex {
    by_bare: IndexMap<String, Vec<UnitOwner>>,
}

impl UnitIndex {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Exactly one owner for `bare`. Missing or multi-owner → `None`.
    #[must_use]
    pub fn unique_owner(&self, bare: &str) -> Option<&Arc<LemmaType>> {
        let owners = self.by_bare.get(bare)?;
        match owners.as_slice() {
            [only] => Some(&only.owning_type),
            _ => None,
        }
    }

    /// True iff [`Self::unique_owner`] would return `Some`.
    #[must_use]
    pub fn has_unique_owner(&self, bare: &str) -> bool {
        self.unique_owner(bare).is_some()
    }

    #[must_use]
    pub fn owners_for(&self, bare: &str) -> &[UnitOwner] {
        self.by_bare.get(bare).map(Vec::as_slice).unwrap_or(&[])
    }

    /// All `(bare, owner)` pairs for signature/decomp builds.
    pub fn iter_entries(&self) -> impl Iterator<Item = (&str, &Arc<LemmaType>)> {
        self.by_bare.iter().flat_map(|(bare, owners)| {
            owners
                .iter()
                .map(move |owner| (bare.as_str(), &owner.owning_type))
        })
    }

    /// Deduplicated owner arcs (by type name + import alias + type pointer identity via name).
    pub fn values(&self) -> impl Iterator<Item = &Arc<LemmaType>> {
        let mut seen = BTreeSet::new();
        self.by_bare
            .values()
            .flat_map(|owners| owners.iter())
            .filter_map(move |owner| {
                let key = (
                    owner.import_alias.clone(),
                    owner.type_name.clone(),
                    owner.owning_type.name(),
                );
                if seen.insert(key) {
                    Some(&owner.owning_type)
                } else {
                    None
                }
            })
    }

    pub fn keys(&self) -> impl Iterator<Item = &String> {
        self.by_bare.keys()
    }

    /// Insert or merge an owner for `bare`. Same type_name + import_alias replaces the arc.
    pub fn insert_owner(&mut self, bare: String, owner: UnitOwner) {
        let owners = self.by_bare.entry(bare).or_default();
        if let Some(existing) = owners.iter_mut().find(|existing| {
            existing.type_name == owner.type_name && existing.import_alias == owner.import_alias
        }) {
            *existing = owner;
            return;
        }
        owners.push(owner);
    }

    /// Merge one measure unit name from `resolved_type` into the index.
    ///
    /// Keeps the declarer as bare owner when a child extends a parent. Rejects a second
    /// independent declarer of the same unit name. Same-family factor checks unchanged.
    pub fn merge_measure_unit(
        &mut self,
        unit: String,
        resolved_type: &Arc<LemmaType>,
        type_name: &str,
        import_alias: Option<String>,
        measure_family: &str,
    ) -> Result<(), UnitMergeConflict> {
        let owners = self.by_bare.entry(unit.clone()).or_default();
        if owners
            .iter()
            .any(|owner| owner.type_name == type_name && owner.import_alias == import_alias)
        {
            return Ok(());
        }

        let resolved_ref = resolved_type.as_ref();
        let mut skip_insert = false;
        let mut reclaim_indices = Vec::new();
        for (index, owner) in owners.iter().enumerate() {
            let existing_type = owner.owning_type.as_ref();
            let existing_name = owner.type_name.as_str();
            let current_extends_existing = resolved_ref
                .extends
                .parent_name()
                .map(|parent| parent == existing_name)
                .unwrap_or(false);
            let existing_extends_current = existing_type
                .extends
                .parent_name()
                .map(|parent| parent == type_name)
                .unwrap_or(false);

            if existing_type.is_measure() && (current_extends_existing || existing_extends_current)
            {
                if current_extends_existing {
                    // Child registering inherited unit; declarer already owns bare name.
                    skip_insert = true;
                } else {
                    // Parent declarer arrived after child claimed inherited unit (HashMap order).
                    reclaim_indices.push(index);
                }
                continue;
            }

            if existing_type.is_ratio() {
                return Err(UnitMergeConflict::Ambiguous {
                    unit,
                    existing_name: existing_name.to_string(),
                    new_name: type_name.to_string(),
                });
            }

            if existing_type.is_measure() && existing_type.same_measure_family(resolved_ref) {
                if let (
                    TypeSpecification::Measure {
                        units: existing_units,
                        ..
                    },
                    TypeSpecification::Measure {
                        units: new_units, ..
                    },
                ) = (&existing_type.specifications, &resolved_ref.specifications)
                {
                    let same_factor = existing_units
                        .iter()
                        .find(|existing_unit| existing_unit.name == unit)
                        .zip(new_units.iter().find(|new_unit| new_unit.name == unit))
                        .is_some_and(|(existing_unit, new_unit)| {
                            existing_unit.factor == new_unit.factor
                        });
                    if same_factor {
                        skip_insert = true;
                        continue;
                    }
                    return Err(UnitMergeConflict::ConflictingFactors {
                        unit,
                        family: measure_family.to_string(),
                    });
                }
            }

            if existing_type.is_measure() {
                return Err(UnitMergeConflict::Ambiguous {
                    unit,
                    existing_name: existing_name.to_string(),
                    new_name: type_name.to_string(),
                });
            }
        }

        for index in reclaim_indices.into_iter().rev() {
            owners.remove(index);
        }

        if !skip_insert {
            owners.push(UnitOwner {
                owning_type: Arc::clone(resolved_type),
                type_name: type_name.to_string(),
                import_alias,
            });
        }
        Ok(())
    }

    /// Merge one ratio unit name from `resolved_type` into the index.
    ///
    /// Replaces a sole primitive-ratio placeholder; rejects cross-kind and conflicting ratios.
    /// Same `(type_name, import_alias)` replaces the arc. A new alias for the same logical
    /// type inserts a second owner so both qualify paths resolve.
    pub fn merge_ratio_unit(
        &mut self,
        unit: String,
        resolved_type: &Arc<LemmaType>,
        type_name: &str,
        import_alias: Option<String>,
        primitive_ratio: &Arc<LemmaType>,
    ) -> Result<(), UnitMergeConflict> {
        let owners = self.by_bare.entry(unit.clone()).or_default();
        if let Some(existing) = owners
            .iter_mut()
            .find(|owner| owner.type_name == type_name && owner.import_alias == import_alias)
        {
            existing.owning_type = Arc::clone(resolved_type);
            return Ok(());
        }

        if owners.is_empty() {
            owners.push(UnitOwner {
                owning_type: Arc::clone(resolved_type),
                type_name: type_name.to_string(),
                import_alias,
            });
            return Ok(());
        }

        if owners.len() == 1 && Arc::ptr_eq(&owners[0].owning_type, primitive_ratio) {
            owners.clear();
            owners.push(UnitOwner {
                owning_type: Arc::clone(resolved_type),
                type_name: type_name.to_string(),
                import_alias,
            });
            return Ok(());
        }

        let resolved_ref = resolved_type.as_ref();
        let mut skip_insert = false;
        for owner in owners.iter() {
            let existing_type = owner.owning_type.as_ref();
            if !existing_type.is_ratio() {
                return Err(UnitMergeConflict::Ambiguous {
                    unit,
                    existing_name: owner.type_name.clone(),
                    new_name: type_name.to_string(),
                });
            }
            if existing_type.name() == resolved_ref.name() {
                continue;
            }
            // Builtin percent/permille are shared across named ratio types (same factors).
            // Custom ratio unit names still have one independent declarer.
            if matches!(unit.as_str(), "percent" | "permille") {
                if let (
                    TypeSpecification::Ratio {
                        units: existing_units,
                        ..
                    },
                    TypeSpecification::Ratio {
                        units: new_units, ..
                    },
                ) = (&existing_type.specifications, &resolved_ref.specifications)
                {
                    let same_factor = existing_units
                        .iter()
                        .find(|existing_unit| existing_unit.name == unit)
                        .zip(new_units.iter().find(|new_unit| new_unit.name == unit))
                        .is_some_and(|(existing_unit, new_unit)| {
                            existing_unit.value == new_unit.value
                        });
                    if same_factor {
                        skip_insert = true;
                        continue;
                    }
                }
            }
            return Err(UnitMergeConflict::AmbiguousRatio {
                unit,
                existing_name: owner.type_name.clone(),
                new_name: type_name.to_string(),
            });
        }
        if !skip_insert {
            owners.push(UnitOwner {
                owning_type: Arc::clone(resolved_type),
                type_name: type_name.to_string(),
                import_alias,
            });
        }
        Ok(())
    }

    /// Consume into `(bare, owner)` pairs.
    pub fn into_iter_owners(self) -> impl Iterator<Item = (String, UnitOwner)> {
        self.by_bare
            .into_iter()
            .flat_map(|(bare, owners)| owners.into_iter().map(move |owner| (bare.clone(), owner)))
    }

    /// Resolve a bare or qualified unit reference.
    ///
    /// Returns `(bare_unit_name, owning_type)`. Err is a message for the caller to wrap in `Error`.
    pub fn resolve(&self, unit_ref: &str) -> Result<(String, Arc<LemmaType>), String> {
        let segments: Vec<String> = unit_ref
            .split('.')
            .map(|segment| crate::parsing::ast::ascii_lowercase_logical_name(segment.to_string()))
            .filter(|segment| !segment.is_empty())
            .collect();
        if segments.is_empty() {
            return Err("Unit path is empty".to_string());
        }
        let bare = segments
            .last()
            .expect("BUG: non-empty segments must have last")
            .clone();
        let owners = self.owners_for(&bare);

        match segments.len() {
            1 => match owners {
                [] => Err(format!(
                    "Unknown unit '{bare}'. Declare it on a measure or ratio type, or import it with uses."
                )),
                [only] => Ok((bare, Arc::clone(&only.owning_type))),
                many => Err(format!(
                    "Unit '{bare}' matches more than one type. Write one of: {}",
                    format_qualifier_list(many, &bare)
                )),
            },
            2 => {
                // Type.unit only. Import-alias sugar (alias.unit) is rejected: use alias.Type.unit.
                let type_name = &segments[0];
                let mut matches: Vec<&UnitOwner> = owners
                    .iter()
                    .filter(|owner| owner.type_name == *type_name)
                    .collect();
                dedupe_owner_matches(&mut matches);
                match matches.as_slice() {
                    [only] => Ok((bare, Arc::clone(&only.owning_type))),
                    [] => Err(format!(
                        "Unknown unit '{unit_ref}'. Use Type.unit or alias.Type.unit \
                         (for example units.mass.kilogram), not alias.unit."
                    )),
                    _ => Err(format!(
                        "Unit '{unit_ref}' matches more than one type. Write one of: {}",
                        format_qualifier_list(owners, &bare)
                    )),
                }
            }
            3 => {
                let alias = &segments[0];
                let type_name = &segments[1];
                let matches: Vec<&UnitOwner> = owners
                    .iter()
                    .filter(|owner| {
                        owner.import_alias.as_deref() == Some(alias.as_str())
                            && owner.type_name == *type_name
                    })
                    .collect();
                match matches.as_slice() {
                    [only] => Ok((bare, Arc::clone(&only.owning_type))),
                    [] => Err(format!(
                        "Unknown unit '{unit_ref}'. Declare it on a measure or ratio type, or import it with uses."
                    )),
                    _ => Err(format!("Unit '{unit_ref}' matches more than one type.")),
                }
            }
            _ => Err(format!(
                "Invalid unit path '{unit_ref}'. Use unit, Type.unit, or alias.Type.unit"
            )),
        }
    }

    /// Type.unit / alias.Type.unit when the named measure inherits the unit but is not the bare owner.
    #[must_use]
    pub fn resolve_via_named_measure_type(
        unit_ref: &str,
        resolved: &IndexMap<String, Arc<LemmaType>>,
    ) -> Option<(String, Arc<LemmaType>)> {
        let segments: Vec<String> = unit_ref
            .split('.')
            .map(|segment| crate::parsing::ast::ascii_lowercase_logical_name(segment.to_string()))
            .filter(|segment| !segment.is_empty())
            .collect();
        let (type_name, bare) = match segments.as_slice() {
            [type_name, bare] => (type_name.as_str(), bare.as_str()),
            [_, type_name, bare] => (type_name.as_str(), bare.as_str()),
            _ => return None,
        };
        let lemma_type = resolved.get(type_name)?;
        if !lemma_type.is_measure() {
            return None;
        }
        let names = lemma_type.measure_unit_names()?;
        if !names.contains(&bare) {
            return None;
        }
        Some((bare.to_string(), Arc::clone(lemma_type)))
    }

    /// Index resolve, then named-measure Type.unit fallback for extension binding.
    pub fn resolve_with_named_types(
        &self,
        unit_ref: &str,
        resolved: &IndexMap<String, Arc<LemmaType>>,
    ) -> Result<(String, Arc<LemmaType>), String> {
        match self.resolve(unit_ref) {
            Ok(hit) => Ok(hit),
            Err(index_err) => {
                Self::resolve_via_named_measure_type(unit_ref, resolved).ok_or(index_err)
            }
        }
    }

    /// Look up a unit factor on candidate measure types, else the unique index owner.
    ///
    /// Multi-owner bare with no declaring typed owner returns `None` (expand leaves the
    /// factor unexpanded). Do not pick an owner.
    #[must_use]
    pub fn owning_type_for_signature_factor<'a>(
        &'a self,
        unit_name: &str,
        typed_owners: &[&'a LemmaType],
    ) -> Option<&'a LemmaType> {
        for typed in typed_owners {
            if type_declares_unit(typed, unit_name) {
                return Some(typed);
            }
        }
        match self.owners_for(unit_name) {
            [only] => Some(only.owning_type.as_ref()),
            _ => None,
        }
    }
}

fn type_declares_unit(lemma_type: &LemmaType, unit_name: &str) -> bool {
    match &lemma_type.specifications {
        crate::planning::semantics::TypeSpecification::Measure { units, .. } => {
            units.iter().any(|unit| unit.name == unit_name)
        }
        crate::planning::semantics::TypeSpecification::Ratio { units, .. } => {
            units.iter().any(|unit| unit.name == unit_name)
        }
        _ => false,
    }
}

fn dedupe_owner_matches(matches: &mut Vec<&UnitOwner>) {
    let mut seen = BTreeSet::new();
    matches.retain(|owner| {
        seen.insert((
            owner.import_alias.clone(),
            owner.type_name.clone(),
            owner.owning_type.name(),
        ))
    });
}

fn format_qualifier_list(owners: &[UnitOwner], bare: &str) -> String {
    let mut paths = BTreeSet::new();
    for owner in owners {
        paths.insert(format!("{}.{}", owner.type_name, bare));
        if let Some(alias) = &owner.import_alias {
            paths.insert(format!("{}.{}.{}", alias, owner.type_name, bare));
        }
    }
    paths.into_iter().collect::<Vec<_>>().join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::computation::rational::rational_one;
    use crate::literals::{MeasureUnit, MeasureUnits};
    use crate::planning::semantics::{LemmaType, TypeExtends, TypeSpecification};

    fn measure_type(name: &str, unit: &str) -> Arc<LemmaType> {
        let mut units = MeasureUnits::new();
        units.push(MeasureUnit {
            name: unit.to_string(),
            factor: rational_one(),
            derived_measure_factors: Vec::new(),
            decomposition: Default::default(),
            minimum: None,
            maximum: None,
            suggestion_magnitude: None,
        });
        Arc::new(LemmaType::new(
            name.to_string(),
            TypeSpecification::Measure {
                units,
                decimals: None,
                traits: vec![],
                decomposition: None,
                minimum: None,
                maximum: None,
                unit: None,
                help: String::new(),
            },
            TypeExtends::Primitive,
        ))
    }

    #[test]
    fn merge_parent_reclaims_inherited_unit_from_child() {
        let mut index = UnitIndex::new();
        let money = measure_type("money", "eur");
        let money2 = Arc::new(LemmaType::new(
            "money2".to_string(),
            money.specifications.clone(),
            TypeExtends::custom_local("money".to_string(), "money".to_string()),
        ));
        index
            .merge_measure_unit("eur".into(), &money2, "money2", None, "money")
            .expect("child first");
        assert_eq!(
            index.unique_owner("eur").map(|t| t.name()),
            Some("money2".into())
        );
        index
            .merge_measure_unit("eur".into(), &money, "money", None, "money")
            .expect("parent reclaim");
        assert_eq!(
            index.unique_owner("eur").map(|t| t.name()),
            Some("money".into()),
            "declarer must reclaim bare ownership"
        );
    }

    #[test]
    fn bare_unique_resolves() {
        let mut index = UnitIndex::new();
        let mass = measure_type("mass", "kilogram");
        index.insert_owner(
            "kilogram".into(),
            UnitOwner {
                owning_type: Arc::clone(&mass),
                type_name: "mass".into(),
                import_alias: Some("units".into()),
            },
        );
        let (bare, owner) = index.resolve("kilogram").expect("unique");
        assert_eq!(bare, "kilogram");
        assert_eq!(owner.name(), "mass");
        let sugar_err = index
            .resolve("units.kilogram")
            .expect_err("import-alias sugar must be rejected");
        assert!(
            sugar_err.contains("Unknown") || sugar_err.contains("alias.Type.unit"),
            "got: {sugar_err}"
        );
        let (bare, owner) = index.resolve("units.mass.kilogram").expect("full");
        assert_eq!(bare, "kilogram");
        assert_eq!(owner.name(), "mass");
        let (bare, owner) = index.resolve("mass.kilogram").expect("type.unit");
        assert_eq!(bare, "kilogram");
        assert_eq!(owner.name(), "mass");
    }

    #[test]
    fn two_segment_import_alias_unit_rejected() {
        let mut index = UnitIndex::new();
        let mass = measure_type("mass", "kilogram");
        index.insert_owner(
            "kilogram".into(),
            UnitOwner {
                owning_type: Arc::clone(&mass),
                type_name: "mass".into(),
                import_alias: Some("units".into()),
            },
        );
        assert!(
            index.resolve("units.kilogram").is_err(),
            "alias.unit sugar rejected even when unique under alias"
        );
        index
            .resolve("units.mass.kilogram")
            .expect("alias.Type.unit must resolve");
    }

    #[test]
    fn merge_second_measure_declarer_rejected() {
        let mut index = UnitIndex::new();
        let a = measure_type("money_a", "eur");
        let b = measure_type("money_b", "eur");
        index
            .merge_measure_unit("eur".into(), &a, "money_a", None, "money_a")
            .expect("first declarer");
        let err = index
            .merge_measure_unit("eur".into(), &b, "money_b", None, "money_b")
            .expect_err("second independent eur declarer must Err");
        match err {
            UnitMergeConflict::Ambiguous {
                unit,
                existing_name,
                new_name,
            } => {
                assert_eq!(unit, "eur");
                assert!(
                    (existing_name == "money_a" && new_name == "money_b")
                        || (existing_name == "money_b" && new_name == "money_a"),
                    "got {existing_name} / {new_name}"
                );
            }
            other => panic!("expected Ambiguous, got {other:?}"),
        }
        assert!(
            index.has_unique_owner("eur"),
            "index must keep sole first declarer after rejected merge"
        );
    }

    #[test]
    fn ratio_merge_inserts_second_alias_for_same_type() {
        let mut index = UnitIndex::new();
        let ratio = {
            let mut units = crate::literals::RatioUnits::new();
            units.push(crate::literals::RatioUnit {
                name: "percent".into(),
                value: rational_one(),
                minimum: None,
                maximum: None,
                suggestion_magnitude: None,
            });
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
                TypeExtends::Primitive,
            ))
        };
        let primitive_placeholder = measure_type("unused_primitive_placeholder", "x");
        index
            .merge_ratio_unit(
                "percent".into(),
                &ratio,
                "rate",
                Some("alpha".into()),
                &primitive_placeholder,
            )
            .expect("first alias");
        index
            .merge_ratio_unit(
                "percent".into(),
                &ratio,
                "rate",
                Some("beta".into()),
                &primitive_placeholder,
            )
            .expect("second alias");
        assert_eq!(index.owners_for("percent").len(), 2);
        assert!(
            index.resolve("alpha.percent").is_err(),
            "alias.unit sugar rejected for ratio units"
        );
        assert!(index.resolve("beta.percent").is_err());
        index
            .resolve("alpha.rate.percent")
            .expect("alpha.Type.unit");
        index.resolve("beta.rate.percent").expect("beta.Type.unit");
        let type_unit_err = index
            .resolve("rate.percent")
            .expect_err("Type.unit ambiguous when same type_name appears under two aliases");
        assert!(
            type_unit_err.contains("more than one type")
                || type_unit_err.contains("Write one of")
                || type_unit_err.contains("ambiguous")
                || type_unit_err.contains("Qualify"),
            "got: {type_unit_err}"
        );
    }
}
