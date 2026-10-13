//! Expression-scope unit index: every declarer of each bare unit name in scope.
//! Qualify with `Type.unit`, `alias.Type.unit`, nested import paths, or `alias.unit`
//! when that import scope identifies the owner.

use crate::planning::semantics::{primitive_ratio_arc, LemmaType, SpecIdentity, TypeSpecification};
use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeSet, HashMap};
use std::sync::Arc;

/// Owner recorded while types are still being resolved. Local types are names only;
/// imported types keep the Arc the other spec already finalized.
#[derive(Debug, Clone)]
pub enum PlanningOwner {
    PrimitiveRatio,
    Local {
        type_name: String,
    },
    Imported {
        import_path: Vec<String>,
        type_name: String,
        lemma_type: Arc<LemmaType>,
    },
}

impl PlanningOwner {
    fn type_name(&self) -> &str {
        match self {
            Self::PrimitiveRatio => "ratio",
            Self::Local { type_name } | Self::Imported { type_name, .. } => type_name,
        }
    }

    fn import_path(&self) -> &[String] {
        match self {
            Self::PrimitiveRatio | Self::Local { .. } => &[],
            Self::Imported { import_path, .. } => import_path.as_slice(),
        }
    }

    fn same_slot(&self, type_name: &str, import_path: &[String]) -> bool {
        match self {
            Self::PrimitiveRatio => false,
            Self::Local {
                type_name: existing,
            } => import_path.is_empty() && existing == type_name,
            Self::Imported {
                import_path: existing_path,
                type_name: existing,
                ..
            } => existing_path.as_slice() == import_path && existing == type_name,
        }
    }

    /// Local bodies come from `resolved`. Imported bodies are the other spec's final Arc.
    pub fn lemma_type<'a>(
        &'a self,
        resolved: &'a IndexMap<String, Arc<LemmaType>>,
    ) -> &'a LemmaType {
        match self {
            Self::PrimitiveRatio => primitive_ratio_arc().as_ref(),
            Self::Local { type_name } => resolved
                .get(type_name)
                .expect("BUG: local unit owner missing from resolved")
                .as_ref(),
            Self::Imported { lemma_type, .. } => lemma_type.as_ref(),
        }
    }
}

/// Bound owner after magnitudes: eval and Show read Arcs from here.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum UnitOwner {
    PrimitiveRatio,
    Named {
        import_path: Vec<String>,
        type_name: String,
        #[serde(with = "crate::snapshot::shared::arc")]
        lemma_type: Arc<LemmaType>,
    },
}

impl UnitOwner {
    #[must_use]
    pub fn lemma_type(&self) -> &Arc<LemmaType> {
        match self {
            Self::PrimitiveRatio => primitive_ratio_arc(),
            Self::Named { lemma_type, .. } => lemma_type,
        }
    }

    #[must_use]
    pub fn type_name(&self) -> &str {
        match self {
            Self::PrimitiveRatio => "ratio",
            Self::Named { type_name, .. } => type_name,
        }
    }

    #[must_use]
    pub fn import_path(&self) -> &[String] {
        match self {
            Self::PrimitiveRatio => &[],
            Self::Named { import_path, .. } => import_path.as_slice(),
        }
    }
}

/// Conflict while merging a unit owner into the index during type resolution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UnitMergeConflict {
    ConflictingFactors { unit: String, family: String },
}

/// Names of unit owners while `resolved` is still being mutated.
#[derive(Debug, Clone, Default)]
pub struct PlanningUnitIndex {
    by_bare: IndexMap<String, Vec<PlanningOwner>>,
}

impl PlanningUnitIndex {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn has_unique_owner(&self, bare: &str) -> bool {
        matches!(self.owners_for(bare), [_])
    }

    #[must_use]
    pub fn owners_for(&self, bare: &str) -> &[PlanningOwner] {
        self.by_bare.get(bare).map(Vec::as_slice).unwrap_or(&[])
    }

    pub fn iter_owners(&self) -> impl Iterator<Item = (&str, &PlanningOwner)> {
        self.by_bare
            .iter()
            .flat_map(|(bare, owners)| owners.iter().map(move |owner| (bare.as_str(), owner)))
    }

    pub fn insert_owner(&mut self, bare: String, owner: PlanningOwner) {
        let owners = self.by_bare.entry(bare).or_default();
        if let Some(existing) = owners
            .iter_mut()
            .find(|existing| planning_owners_same_slot(existing, &owner))
        {
            *existing = owner;
            return;
        }
        owners.push(owner);
    }

    pub fn merge_measure_unit(
        &mut self,
        unit: String,
        resolved_type: &Arc<LemmaType>,
        type_name: &str,
        import_path: &[String],
        measure_family: &str,
        locals: &HashMap<String, Arc<LemmaType>>,
    ) -> Result<(), UnitMergeConflict> {
        let owners = self.by_bare.entry(unit.clone()).or_default();
        if owners
            .iter()
            .any(|owner| owner.same_slot(type_name, import_path))
        {
            return Ok(());
        }

        let resolved_ref = resolved_type.as_ref();
        let mut skip_insert = false;
        let mut reclaim_indices = Vec::new();
        for (index, owner) in owners.iter().enumerate() {
            let existing_type = planning_owner_type(owner, locals);
            let existing_name = owner.type_name();
            let existing_path = owner.import_path();
            let parent_ref = resolved_ref.extends.parent_name();
            let current_extends_existing = parent_ref
                .is_some_and(|parent| parent_matches_owner(parent, existing_path, existing_name));
            let existing_extends_current = existing_type
                .extends
                .parent_name()
                .is_some_and(|parent| parent_matches_owner(parent, import_path, type_name));

            if existing_type.is_measure() && (current_extends_existing || existing_extends_current)
            {
                if current_extends_existing {
                    skip_insert = true;
                } else {
                    reclaim_indices.push(index);
                }
                continue;
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
                        // Same import path family: keep one local declarer (extension chain).
                        // Distinct import paths keep every owner; bare use collapses later.
                        if import_path.is_empty() && existing_path.is_empty() {
                            skip_insert = true;
                        }
                        continue;
                    }
                    return Err(UnitMergeConflict::ConflictingFactors {
                        unit,
                        family: measure_family.to_string(),
                    });
                }
            }
        }

        for index in reclaim_indices.into_iter().rev() {
            owners.remove(index);
        }

        if !skip_insert {
            owners.push(planning_named_owner(type_name, import_path, resolved_type));
        }
        Ok(())
    }

    pub fn merge_ratio_unit(
        &mut self,
        unit: String,
        resolved_type: &Arc<LemmaType>,
        type_name: &str,
        import_path: &[String],
        locals: &HashMap<String, Arc<LemmaType>>,
    ) -> Result<(), UnitMergeConflict> {
        let owners = self.by_bare.entry(unit.clone()).or_default();
        if let Some(existing) = owners
            .iter_mut()
            .find(|owner| owner.same_slot(type_name, import_path))
        {
            if let PlanningOwner::Imported { lemma_type, .. } = existing {
                *lemma_type = Arc::clone(resolved_type);
            }
            return Ok(());
        }

        if owners.is_empty() {
            owners.push(planning_named_owner(type_name, import_path, resolved_type));
            return Ok(());
        }

        // Keep PrimitiveRatio for bare percent/permille; named owners stay for Type.unit.
        let resolved_ref = resolved_type.as_ref();
        let mut skip_insert = false;
        for owner in owners.iter() {
            if matches!(owner, PlanningOwner::PrimitiveRatio) {
                continue;
            }
            let existing_type = planning_owner_type(owner, locals);
            if !existing_type.is_ratio() || existing_type.name() == resolved_ref.name() {
                continue;
            }
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
                    if same_factor && import_path.is_empty() && owner.import_path().is_empty() {
                        skip_insert = true;
                    }
                }
            }
        }
        if !skip_insert {
            owners.push(planning_named_owner(type_name, import_path, resolved_type));
        }
        Ok(())
    }

    pub fn resolve_owner(
        &self,
        unit_ref: &str,
        resolved: &IndexMap<String, Arc<LemmaType>>,
    ) -> Result<(String, &PlanningOwner), String> {
        let bare_key = unit_ref_bare(unit_ref);
        let owners = self.owners_for(&bare_key);
        resolve_owners_for_ref(
            owners,
            unit_ref,
            |owner| owner.type_name(),
            |owner| owner.import_path(),
            |owner| planning_owner_lemma_arc(owner, resolved),
        )
    }

    pub fn resolve_with_named_types(
        &self,
        unit_ref: &str,
        resolved: &IndexMap<String, Arc<LemmaType>>,
    ) -> Result<(String, Arc<LemmaType>), String> {
        match self.resolve_owner(unit_ref, resolved) {
            Ok((bare, owner)) => Ok((
                bare,
                Arc::clone(match owner {
                    PlanningOwner::PrimitiveRatio => primitive_ratio_arc(),
                    PlanningOwner::Local { type_name } => resolved
                        .get(type_name)
                        .expect("BUG: local unit owner missing from resolved"),
                    PlanningOwner::Imported { lemma_type, .. } => lemma_type,
                }),
            )),
            Err(index_err) => {
                UnitIndex::resolve_via_named_measure_type(unit_ref, resolved).ok_or(index_err)
            }
        }
    }

    pub fn into_iter_owners(self) -> impl Iterator<Item = (String, PlanningOwner)> {
        self.by_bare
            .into_iter()
            .flat_map(|(bare, owners)| owners.into_iter().map(move |owner| (bare.clone(), owner)))
    }

    /// Attach local Arcs from final `resolved` and add missing family-root units.
    #[must_use]
    pub fn bind(
        self,
        resolved: &IndexMap<String, Arc<LemmaType>>,
        this_spec: &SpecIdentity,
    ) -> UnitIndex {
        let mut bound = UnitIndex::new();
        for (unit_name, owner) in self.into_iter_owners() {
            let bound_owner = match owner {
                PlanningOwner::PrimitiveRatio => UnitOwner::PrimitiveRatio,
                PlanningOwner::Local { type_name } => {
                    let lemma_type = Arc::clone(
                        resolved
                            .get(&type_name)
                            .expect("BUG: local unit owner missing from resolved"),
                    );
                    if !type_declares_unit(lemma_type.as_ref(), &unit_name) {
                        continue;
                    }
                    UnitOwner::Named {
                        import_path: Vec::new(),
                        type_name,
                        lemma_type,
                    }
                }
                PlanningOwner::Imported {
                    import_path,
                    type_name,
                    lemma_type,
                } => {
                    if !type_declares_unit(lemma_type.as_ref(), &unit_name) {
                        continue;
                    }
                    UnitOwner::Named {
                        import_path,
                        type_name,
                        lemma_type,
                    }
                }
            };
            bound.insert_owner(unit_name, bound_owner);
        }
        for (type_name, lemma_type) in resolved {
            let TypeSpecification::Measure { units, .. } = &lemma_type.specifications else {
                continue;
            };
            let is_family_root = lemma_type.measure_family().is_some_and(|family| {
                family.is_declared_in(this_spec) && family.name == *type_name
            });
            if !is_family_root {
                continue;
            }
            for unit in units.iter() {
                let already = bound
                    .owners_for(&unit.name)
                    .iter()
                    .any(|owner| match owner {
                        UnitOwner::Named {
                            type_name: owner_name,
                            import_path,
                            ..
                        } if import_path.is_empty() && owner_name == type_name => true,
                        UnitOwner::Named {
                            lemma_type: owner_type,
                            ..
                        } => owner_type.same_measure_family(lemma_type.as_ref()),
                        UnitOwner::PrimitiveRatio => false,
                    });
                if !already {
                    bound.insert_owner(
                        unit.name.clone(),
                        UnitOwner::Named {
                            import_path: Vec::new(),
                            type_name: type_name.clone(),
                            lemma_type: Arc::clone(lemma_type),
                        },
                    );
                }
            }
        }
        bound
    }
}

/// Expression-scope units after bind: every declarer per bare unit name.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct UnitIndex {
    by_bare: IndexMap<String, Vec<UnitOwner>>,
}

impl UnitIndex {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn unique_owner(&self, bare: &str) -> Option<&Arc<LemmaType>> {
        let owners = self.by_bare.get(bare)?;
        match owners.as_slice() {
            [only] => Some(only.lemma_type()),
            _ => None,
        }
    }

    #[must_use]
    pub fn owners_for(&self, bare: &str) -> &[UnitOwner] {
        self.by_bare.get(bare).map(Vec::as_slice).unwrap_or(&[])
    }

    pub fn iter_entries(&self) -> impl Iterator<Item = (&str, &Arc<LemmaType>)> {
        self.by_bare.iter().flat_map(|(bare, owners)| {
            owners
                .iter()
                .map(move |owner| (bare.as_str(), owner.lemma_type()))
        })
    }

    pub fn iter_owners(&self) -> impl Iterator<Item = (&str, &UnitOwner)> {
        self.by_bare
            .iter()
            .flat_map(|(bare, owners)| owners.iter().map(move |owner| (bare.as_str(), owner)))
    }

    pub fn values(&self) -> impl Iterator<Item = &Arc<LemmaType>> {
        let mut seen = BTreeSet::new();
        self.by_bare
            .values()
            .flat_map(|owners| owners.iter())
            .filter_map(move |owner| {
                let key = (
                    owner.import_path().to_vec(),
                    owner.type_name().to_string(),
                    owner.lemma_type().name(),
                );
                if seen.insert(key) {
                    Some(owner.lemma_type())
                } else {
                    None
                }
            })
    }

    pub fn keys(&self) -> impl Iterator<Item = &String> {
        self.by_bare.keys()
    }

    pub fn insert_owner(&mut self, bare: String, owner: UnitOwner) {
        let owners = self.by_bare.entry(bare).or_default();
        if let Some(existing) = owners
            .iter_mut()
            .find(|existing| bound_owners_same_slot(existing, &owner))
        {
            *existing = owner;
            return;
        }
        owners.push(owner);
    }

    pub fn resolve(&self, unit_ref: &str) -> Result<(String, Arc<LemmaType>), String> {
        self.resolve_owner(unit_ref)
            .map(|(bare, owner)| (bare, Arc::clone(owner.lemma_type())))
    }

    pub fn resolve_owner(&self, unit_ref: &str) -> Result<(String, &UnitOwner), String> {
        resolve_owners_for_ref(
            self.owners_for(&unit_ref_bare(unit_ref)),
            unit_ref,
            |owner| owner.type_name(),
            |owner| owner.import_path(),
            |owner| Arc::clone(owner.lemma_type()),
        )
    }

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
            [only] => Some(only.lemma_type().as_ref()),
            _ => None,
        }
    }
}

fn planning_named_owner(
    type_name: &str,
    import_path: &[String],
    resolved_type: &Arc<LemmaType>,
) -> PlanningOwner {
    if import_path.is_empty() {
        PlanningOwner::Local {
            type_name: type_name.to_string(),
        }
    } else {
        PlanningOwner::Imported {
            import_path: import_path.to_vec(),
            type_name: type_name.to_string(),
            lemma_type: Arc::clone(resolved_type),
        }
    }
}

fn parent_matches_owner(parent: &str, owner_path: &[String], owner_type_name: &str) -> bool {
    if owner_path.is_empty() {
        parent == owner_type_name
    } else {
        let mut segments = owner_path.to_vec();
        segments.push(owner_type_name.to_string());
        segments.join(".") == parent
    }
}

fn planning_owner_type<'a>(
    owner: &'a PlanningOwner,
    locals: &'a HashMap<String, Arc<LemmaType>>,
) -> &'a LemmaType {
    match owner {
        PlanningOwner::PrimitiveRatio => primitive_ratio_arc().as_ref(),
        PlanningOwner::Local { type_name } => locals
            .get(type_name)
            .expect("BUG: local unit owner missing from resolved")
            .as_ref(),
        PlanningOwner::Imported { lemma_type, .. } => lemma_type.as_ref(),
    }
}

fn planning_owner_lemma_arc(
    owner: &PlanningOwner,
    resolved: &IndexMap<String, Arc<LemmaType>>,
) -> Arc<LemmaType> {
    match owner {
        PlanningOwner::PrimitiveRatio => Arc::clone(primitive_ratio_arc()),
        PlanningOwner::Local { type_name } => Arc::clone(
            resolved
                .get(type_name)
                .expect("BUG: local unit owner missing from resolved"),
        ),
        PlanningOwner::Imported { lemma_type, .. } => Arc::clone(lemma_type),
    }
}

fn type_declares_unit(lemma_type: &LemmaType, unit_name: &str) -> bool {
    match &lemma_type.specifications {
        TypeSpecification::Measure { units, .. } => units.get(unit_name).is_ok(),
        TypeSpecification::Ratio { units, .. } => units.get(unit_name).is_ok(),
        _ => false,
    }
}

fn planning_owners_same_slot(left: &PlanningOwner, right: &PlanningOwner) -> bool {
    match (left, right) {
        (PlanningOwner::PrimitiveRatio, PlanningOwner::PrimitiveRatio) => true,
        (PlanningOwner::Local { type_name: a }, PlanningOwner::Local { type_name: b }) => a == b,
        (
            PlanningOwner::Imported {
                import_path: path_a,
                type_name: name_a,
                ..
            },
            PlanningOwner::Imported {
                import_path: path_b,
                type_name: name_b,
                ..
            },
        ) => path_a == path_b && name_a == name_b,
        _ => false,
    }
}

fn bound_owners_same_slot(left: &UnitOwner, right: &UnitOwner) -> bool {
    match (left, right) {
        (UnitOwner::PrimitiveRatio, UnitOwner::PrimitiveRatio) => true,
        (
            UnitOwner::Named {
                type_name: name_a,
                import_path: path_a,
                ..
            },
            UnitOwner::Named {
                type_name: name_b,
                import_path: path_b,
                ..
            },
        ) => name_a == name_b && path_a == path_b,
        _ => false,
    }
}

fn unit_ref_bare(unit_ref: &str) -> String {
    unit_ref
        .split('.')
        .map(|segment| crate::parsing::ast::ascii_lowercase_logical_name(segment.to_string()))
        .rfind(|segment| !segment.is_empty())
        .unwrap_or_default()
}

fn unit_scalar_for_bare(lemma_type: &LemmaType, bare: &str) -> Option<UnitScalar> {
    match &lemma_type.specifications {
        TypeSpecification::Measure { units, .. } => units
            .get(bare)
            .ok()
            .map(|unit| UnitScalar::Measure(unit.factor.clone())),
        TypeSpecification::Ratio { units, .. } => units
            .get(bare)
            .ok()
            .map(|unit| UnitScalar::Ratio(unit.value.clone())),
        _ => None,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum UnitScalar {
    Measure(crate::computation::rational::RationalInteger),
    Ratio(crate::computation::rational::RationalInteger),
}

fn try_collapse_equivalent_owners<'a, O>(
    candidates: &[&'a O],
    bare: &str,
    lemma_type: &impl Fn(&O) -> Arc<LemmaType>,
) -> Result<&'a O, ()> {
    let first = *candidates
        .first()
        .expect("BUG: collapse requires non-empty candidates");
    let first_type = lemma_type(first);
    if !type_declares_unit(first_type.as_ref(), bare) {
        return Err(());
    }
    let first_scalar = unit_scalar_for_bare(first_type.as_ref(), bare).ok_or(())?;
    for owner in candidates.iter().copied().skip(1) {
        let other_type = lemma_type(owner);
        let same_root = first_type.as_ref() == other_type.as_ref()
            || first_type.same_measure_family(other_type.as_ref())
            || (first_type.is_ratio()
                && other_type.is_ratio()
                && first_type.ratio_family().is_some()
                && first_type.ratio_family() == other_type.ratio_family());
        if !same_root {
            return Err(());
        }
        if unit_scalar_for_bare(other_type.as_ref(), bare) != Some(first_scalar.clone()) {
            return Err(());
        }
    }
    Ok(first)
}

fn resolve_owners_for_ref<'a, O>(
    owners: &'a [O],
    unit_ref: &str,
    type_name: impl Fn(&O) -> &str + Copy,
    import_path: impl Fn(&O) -> &[String] + Copy,
    lemma_type: impl Fn(&O) -> Arc<LemmaType>,
) -> Result<(String, &'a O), String> {
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
    let prefix = &segments[..segments.len() - 1];
    if prefix.is_empty() {
        // Builtin percent/permille always bind PrimitiveRatio; named owners stay for Type.unit.
        if matches!(bare.as_str(), "percent" | "permille") {
            if let Some(primitive) = owners
                .iter()
                .find(|owner| Arc::ptr_eq(&lemma_type(owner), primitive_ratio_arc()))
            {
                return Ok((bare, primitive));
            }
        }
        return match owners {
            [] => Err(format!(
                "Unknown unit '{bare}'. Declare it on a measure or ratio type, or import it with uses."
            )),
            [only] => Ok((bare, only)),
            many => {
                let refs: Vec<&O> = many.iter().collect();
                if let Ok(only) = try_collapse_equivalent_owners(&refs, &bare, &lemma_type) {
                    Ok((bare, only))
                } else {
                    Err(format!(
                        "Unit '{bare}' matches more than one type. Write one of: {}",
                        format_qualifier_list(many, &bare, type_name, import_path)
                    ))
                }
            }
        };
    }

    let mut matches = owners_matching_prefix(owners, prefix, type_name, import_path);
    dedupe_owner_matches(&mut matches, type_name, import_path);
    match matches.as_slice() {
        [only] => Ok((bare, *only)),
        [] => Err(format!(
            "Unknown unit '{unit_ref}'. Declare it on a measure or ratio type, or import it with uses."
        )),
        many => {
            if let Ok(only) = try_collapse_equivalent_owners(many, &bare, &lemma_type) {
                Ok((bare, only))
            } else {
                Err(format!(
                    "Unit '{unit_ref}' matches more than one type. Write one of: {}",
                    format_qualifier_list_from_refs(many, &bare, type_name, import_path)
                ))
            }
        }
    }
}

fn owners_matching_prefix<'a, O>(
    owners: &'a [O],
    prefix: &[String],
    type_name: impl Fn(&O) -> &str + Copy,
    import_path: impl Fn(&O) -> &[String] + Copy,
) -> Vec<&'a O> {
    if prefix.len() == 1 {
        let want_type = &prefix[0];
        let local: Vec<&O> = owners
            .iter()
            .filter(|owner| import_path(owner).is_empty() && type_name(owner) == want_type)
            .collect();
        if !local.is_empty() {
            return local;
        }
        return owners
            .iter()
            .filter(|owner| {
                let path = import_path(owner);
                !path.is_empty() && path[0] == *want_type
            })
            .collect();
    }

    if prefix.len() >= 2 {
        let type_segment = &prefix[prefix.len() - 1];
        let path_prefix = &prefix[..prefix.len() - 1];
        let exact: Vec<&O> = owners
            .iter()
            .filter(|owner| import_path(owner) == path_prefix && type_name(owner) == type_segment)
            .collect();
        if !exact.is_empty() {
            return exact;
        }
    }

    owners
        .iter()
        .filter(|owner| {
            let path = import_path(owner);
            path.len() >= prefix.len() && path[..prefix.len()] == prefix[..]
        })
        .collect()
}

fn dedupe_owner_matches<O>(
    matches: &mut Vec<&O>,
    type_name: impl Fn(&O) -> &str,
    import_path: impl Fn(&O) -> &[String],
) {
    let mut seen = BTreeSet::new();
    matches
        .retain(|owner| seen.insert((import_path(owner).to_vec(), type_name(owner).to_string())));
}

fn format_qualifier_list_from_refs<O>(
    owners: &[&O],
    bare: &str,
    type_name: impl Fn(&O) -> &str,
    import_path: impl Fn(&O) -> &[String],
) -> String {
    let mut paths = BTreeSet::new();
    for owner in owners {
        let path = import_path(owner);
        let type_label = type_name(owner);
        if path.is_empty() {
            paths.insert(format!("{type_label}.{bare}"));
        } else {
            let full = path
                .iter()
                .chain(std::iter::once(&type_label.to_string()))
                .cloned()
                .collect::<Vec<_>>()
                .join(".");
            paths.insert(format!("{full}.{bare}"));
            let path_only_unique = owners
                .iter()
                .filter(|other| import_path(other) == path)
                .count()
                == 1;
            if path_only_unique {
                paths.insert(format!("{}.{bare}", path.join(".")));
            }
        }
    }
    paths.into_iter().collect::<Vec<_>>().join(", ")
}

fn format_qualifier_list<O>(
    owners: &[O],
    bare: &str,
    type_name: impl Fn(&O) -> &str,
    import_path: impl Fn(&O) -> &[String],
) -> String {
    let mut paths = BTreeSet::new();
    for owner in owners {
        let path = import_path(owner);
        let type_label = type_name(owner);
        if path.is_empty() {
            paths.insert(format!("{type_label}.{bare}"));
        } else {
            let full = path
                .iter()
                .chain(std::iter::once(&type_label.to_string()))
                .cloned()
                .collect::<Vec<_>>()
                .join(".");
            paths.insert(format!("{full}.{bare}"));
            let path_only_unique = owners
                .iter()
                .filter(|other| import_path(other) == path)
                .count()
                == 1;
            if path_only_unique {
                paths.insert(format!("{}.{bare}", path.join(".")));
            }
        }
    }
    paths.into_iter().collect::<Vec<_>>().join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::computation::rational::rational_one;
    use crate::literals::{MeasureUnit, MeasureUnits};
    use crate::parsing::ast::EffectiveDate;
    use crate::planning::semantics::{LemmaType, SpecIdentity, TypeExtends, TypeSpecification};
    use indexmap::IndexMap;
    use std::collections::HashMap;
    use std::sync::Arc;

    fn test_spec() -> SpecIdentity {
        SpecIdentity {
            repository: None,
            spec: "test".into(),
            effective: EffectiveDate::Origin,
        }
    }

    fn locals(pairs: &[(&str, &Arc<LemmaType>)]) -> HashMap<String, Arc<LemmaType>> {
        pairs
            .iter()
            .map(|(name, lemma_type)| ((*name).to_string(), Arc::clone(lemma_type)))
            .collect()
    }

    fn bind_locals(planning: PlanningUnitIndex, pairs: &[(&str, &Arc<LemmaType>)]) -> UnitIndex {
        let mut resolved = IndexMap::new();
        for (name, lemma_type) in pairs {
            resolved.insert((*name).to_string(), Arc::clone(lemma_type));
        }
        planning.bind(&resolved, &test_spec())
    }

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
        let mut index = PlanningUnitIndex::new();
        let money = measure_type("money", "eur");
        let money2 = Arc::new(LemmaType::new(
            "money2".to_string(),
            money.specifications.clone(),
            TypeExtends::custom_local(
                "money".to_string(),
                crate::planning::semantics::tests::test_type_family("money"),
            ),
        ));
        index
            .merge_measure_unit(
                "eur".into(),
                &money2,
                "money2",
                &[],
                "money",
                &locals(&[("money2", &money2)]),
            )
            .expect("child first");
        let bound = bind_locals(index.clone(), &[("money2", &money2)]);
        assert_eq!(
            bound.unique_owner("eur").map(|t| t.name()),
            Some("money2".into())
        );
        index
            .merge_measure_unit(
                "eur".into(),
                &money,
                "money",
                &[],
                "money",
                &locals(&[("money2", &money2), ("money", &money)]),
            )
            .expect("parent reclaim");
        let bound = bind_locals(index, &[("money2", &money2), ("money", &money)]);
        assert_eq!(
            bound.unique_owner("eur").map(|t| t.name()),
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
            UnitOwner::Named {
                lemma_type: Arc::clone(&mass),
                import_path: vec!["units".into()],
                type_name: "mass".into(),
            },
        );
        let (bare, owner) = index.resolve("kilogram").expect("unique");
        assert_eq!(bare, "kilogram");
        assert_eq!(owner.name(), "mass");
        let (bare, owner) = index.resolve("units.kilogram").expect("alias.unit sugar");
        assert_eq!(bare, "kilogram");
        assert_eq!(owner.name(), "mass");
        let (bare, owner) = index.resolve("units.mass.kilogram").expect("full");
        assert_eq!(bare, "kilogram");
        assert_eq!(owner.name(), "mass");
        index
            .resolve("mass.kilogram")
            .expect_err("Type.unit is local-only for imported owner");
    }

    #[test]
    fn two_segment_import_alias_unit_resolves() {
        let mut index = UnitIndex::new();
        let mass = measure_type("mass", "kilogram");
        index.insert_owner(
            "kilogram".into(),
            UnitOwner::Named {
                lemma_type: Arc::clone(&mass),
                import_path: vec!["units".into()],
                type_name: "mass".into(),
            },
        );
        index
            .resolve("units.kilogram")
            .expect("alias.unit resolves when unique under alias");
        index
            .resolve("units.mass.kilogram")
            .expect("alias.Type.unit must resolve");
    }

    #[test]
    fn nested_import_path_type_unit_resolves() {
        let mut index = UnitIndex::new();
        let mass = measure_type("mass", "kilogram");
        index.insert_owner(
            "kilogram".into(),
            UnitOwner::Named {
                lemma_type: Arc::clone(&mass),
                import_path: vec!["a".into(), "b".into()],
                type_name: "mass".into(),
            },
        );
        index
            .resolve("a.b.mass.kilogram")
            .expect("nested alias.Type.unit");
    }

    #[test]
    fn merge_second_measure_declarer_keeps_both_and_bare_use_must_qualify() {
        let mut index = PlanningUnitIndex::new();
        let a = measure_type("money_a", "eur");
        let b = measure_type("money_b", "eur");
        index
            .merge_measure_unit(
                "eur".into(),
                &a,
                "money_a",
                &[],
                "money_a",
                &locals(&[("money_a", &a)]),
            )
            .expect("first declarer");
        index
            .merge_measure_unit(
                "eur".into(),
                &b,
                "money_b",
                &[],
                "money_b",
                &locals(&[("money_a", &a), ("money_b", &b)]),
            )
            .expect("second independent eur declarer is kept");
        assert_eq!(index.owners_for("eur").len(), 2);

        let bound = bind_locals(index, &[("money_a", &a), ("money_b", &b)]);
        let err = bound
            .resolve("eur")
            .expect_err("bare eur with two declarers must be qualified");
        assert!(
            err.contains("money_a.eur") && err.contains("money_b.eur"),
            "got: {err}"
        );
        let (bare, owner) = bound.resolve("money_b.eur").expect("qualified eur");
        assert_eq!(bare, "eur");
        assert_eq!(owner.name(), "money_b");
    }

    #[test]
    fn ratio_merge_inserts_second_alias_for_same_type() {
        let mut index = PlanningUnitIndex::new();
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
        index
            .merge_ratio_unit(
                "percent".into(),
                &ratio,
                "rate",
                &["alpha".into()],
                &HashMap::new(),
            )
            .expect("first alias");
        index
            .merge_ratio_unit(
                "percent".into(),
                &ratio,
                "rate",
                &["beta".into()],
                &HashMap::new(),
            )
            .expect("second alias");
        let bound = bind_locals(index, &[]);
        assert_eq!(bound.owners_for("percent").len(), 2);
        bound
            .resolve("alpha.percent")
            .expect("alias.unit sugar for ratio");
        bound.resolve("beta.percent").expect("beta alias.unit");
        bound
            .resolve("alpha.rate.percent")
            .expect("alpha.Type.unit");
        bound.resolve("beta.rate.percent").expect("beta.Type.unit");
        let type_unit_err = bound
            .resolve("rate.percent")
            .expect_err("Type.unit ambiguous when same type_name appears under two aliases");
        assert!(
            type_unit_err.contains("more than one type")
                || type_unit_err.contains("Write one of")
                || type_unit_err.contains("ambiguous")
                || type_unit_err.contains("Qualify")
                || type_unit_err.contains("Unknown"),
            "got: {type_unit_err}"
        );
    }

    #[test]
    fn named_ratio_keeps_primitive_percent_and_bare_picks_it() {
        let mut index = PlanningUnitIndex::new();
        index.insert_owner("percent".into(), PlanningOwner::PrimitiveRatio);
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
                "margin".into(),
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
        index
            .merge_ratio_unit(
                "percent".into(),
                &ratio,
                "margin",
                &[],
                &locals(&[("margin", &ratio)]),
            )
            .expect("named ratio merges beside builtin");
        let bound = bind_locals(index, &[("margin", &ratio)]);
        assert_eq!(bound.owners_for("percent").len(), 2);
        let (bare, owner) = bound.resolve("percent").expect("bare percent");
        assert_eq!(bare, "percent");
        assert!(Arc::ptr_eq(&owner, primitive_ratio_arc()));
        let (_, qualified) = bound.resolve("margin.percent").expect("Type.unit");
        assert!(Arc::ptr_eq(&qualified, &ratio));
    }
}
