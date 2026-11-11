//! Type resolution for Lemma specs.
//!
//! Named types, unit decompositions, signature indexes, and validation of
//! type specifications — extracted from graph construction so Graph wiring
//! stays separate from TypeResolver.

use crate::computation::arithmetic::{SignatureFactor, SignatureIndex};
use crate::computation::rational::{NumericFailure, RationalInteger};
use crate::engine::Context;
use crate::limits::MAX_UNIT_EXPONENT;
use crate::literals::{MeasureUnit, MeasureUnits};
use crate::parsing::ast::{
    self as ast, CommandArg, Constraint, EffectiveDate, LemmaRepository, LemmaSpec, ParentType,
    PrimitiveKind, TypeConstraintCommand, Value,
};
use crate::parsing::source::Source;
use crate::planning::discovery;
use crate::planning::execution_plan;
use crate::planning::instance_list;
use crate::planning::semantics::{
    self, apply_list_with_rows, apply_option_constraint, apply_spec_member_command,
    bound_value_kind_from_raw_suggestion, calendar_decomposition, calendar_unit_factor,
    canonicalize_signature, duration_decomposition, value_kind_matches_spec, BaseMeasureVector,
    BoundValueKind, ConstraintNames, DataPath, LemmaType, ListKey, LiteralValue, PathSegment,
    RawSuggestion, ReferenceTarget, RulePath, SpecIdentity, SpecVersionKey, TypeDefiningSpec,
    TypeExtends, TypeFamily, TypeOptions, TypeSpecification, TypedLiteral, UnitTokenResolver,
    ValueKind,
};
use crate::planning::unit_index::{
    PlanningOwner, PlanningUnitIndex, UnitIndex, UnitMergeConflict, UnitOwner,
};
use crate::Error;
use ast::DataValue as ParsedDataValue;
use indexmap::IndexMap;
use rust_decimal::Decimal;
use std::cmp::Ordering;
use std::collections::{BTreeSet, HashMap, HashSet, VecDeque};
use std::fmt;
use std::sync::Arc;

pub(crate) type ResolvedTypesMap<'a> =
    Vec<(Arc<LemmaRepository>, &'a LemmaSpec, ResolvedSpecTypes)>;

/// Parent lookup result: specs, inherited suggest, inherited fill, parent type.
type ParentLookupOk = (
    TypeSpecification,
    Option<RawSuggestion>,
    Option<RawSuggestion>,
    Arc<LemmaType>,
);

/// Local resolved type entry during topo-sort: type, suggest, fill.
type ResolvedTypeEntry = (Arc<LemmaType>, Option<RawSuggestion>, Option<RawSuggestion>);

fn local_type_entries(resolved_types: &ResolvedSpecTypes) -> HashMap<String, ResolvedTypeEntry> {
    resolved_types
        .resolved
        .iter()
        .map(|(name, lemma_type)| {
            (
                name.clone(),
                (
                    Arc::clone(lemma_type),
                    resolved_types.source_defaults.get(name).cloned(),
                    resolved_types.source_value_defaults.get(name).cloned(),
                ),
            )
        })
        .collect()
}

/// Type name shown in `-> suggest` constraint errors (the declared type, not the data slot).
pub(crate) fn constraint_application_type_name(parent: &ParentType, data_name: &str) -> String {
    let mut current = parent;
    loop {
        match current {
            ParentType::Custom { name, .. } => return name.clone(),
            ParentType::Qualified { inner, .. }
            | ParentType::Ranged { inner, .. }
            | ParentType::List { inner, .. } => current = inner.as_ref(),
            ParentType::Primitive { .. } => return data_name.to_string(),
        }
    }
}

/// Keep a measure literal's written unit when replacing `lemma_type` with a schema type
/// that has the same specifications (or compatible family).
pub(crate) fn preserve_measure_binding(
    schema_type: &Arc<LemmaType>,
    previous: &LemmaType,
) -> Arc<LemmaType> {
    match previous.unit() {
        Some(unit)
            if matches!(
                &schema_type.specifications,
                TypeSpecification::Measure { .. } | TypeSpecification::Ratio { .. }
            ) =>
        {
            Arc::new(schema_type.as_ref().clone().with_unit(unit))
        }
        Some(_) | None => Arc::clone(schema_type),
    }
}

pub(crate) fn coerce_literal_to_schema_type(
    lit: &TypedLiteral,
    schema_type: &Arc<LemmaType>,
) -> Result<TypedLiteral, String> {
    fn range_endpoint_schema_type(schema_type: &LemmaType) -> Option<Arc<LemmaType>> {
        schema_type
            .specifications
            .element_from_range()
            .map(|element_spec| Arc::new(LemmaType::primitive(element_spec)))
    }

    let schema_ref = schema_type.as_ref();
    match (&schema_ref.specifications, &lit.value) {
        (
            TypeSpecification::NumberRange { .. }
            | TypeSpecification::DateRange { .. }
            | TypeSpecification::TimeRange { .. }
            | TypeSpecification::RatioRange { .. }
            | TypeSpecification::MeasureRange { .. },
            ValueKind::Range(left, right),
        ) if !lit
            .lemma_type
            .specifications
            .equal_ignoring_unit(&schema_ref.specifications) =>
        {
            let endpoint_schema_type =
                range_endpoint_schema_type(schema_ref).unwrap_or_else(|| {
                    unreachable!("BUG: range_endpoint_schema_type missing range schema arm")
                });
            let left_schema =
                preserve_measure_binding(&endpoint_schema_type, left.lemma_type.as_ref());
            let right_schema =
                preserve_measure_binding(&endpoint_schema_type, right.lemma_type.as_ref());
            let coerced_left = coerce_scalar_literal(left, &left_schema)?;
            let coerced_right = coerce_scalar_literal(right, &right_schema)?;
            Ok(TypedLiteral {
                value: ValueKind::Range(Box::new(coerced_left), Box::new(coerced_right)),
                lemma_type: Arc::clone(schema_type),
            })
        }
        _ => coerce_scalar_literal(lit, schema_type),
    }
}

/// [`coerce_literal_to_schema_type`] for a literal that is not a range against a
/// range schema: a range endpoint is coerced here, never as a range.
pub(crate) fn coerce_scalar_literal(
    lit: &TypedLiteral,
    schema_type: &Arc<LemmaType>,
) -> Result<TypedLiteral, String> {
    let schema_ref = schema_type.as_ref();
    if lit
        .lemma_type
        .specifications
        .equal_ignoring_unit(&schema_ref.specifications)
    {
        if !value_kind_matches_spec(&lit.value, &schema_ref.specifications) {
            panic!(
                "BUG: LiteralValue value kind {:?} inconsistent with lemma_type {:?}",
                lit.value, lit.lemma_type.specifications
            );
        }
        let mut out = lit.clone();
        out.lemma_type = preserve_measure_binding(schema_type, &lit.lemma_type);
        return Ok(out);
    }
    match (&schema_ref.specifications, &lit.value) {
        (TypeSpecification::Number { .. }, ValueKind::Number(_))
        | (TypeSpecification::Text { .. }, ValueKind::Text(_))
        | (TypeSpecification::Boolean { .. }, ValueKind::Boolean(_))
        | (TypeSpecification::Date { .. }, ValueKind::Date(_))
        | (TypeSpecification::Time { .. }, ValueKind::Time(_)) => {
            let mut out = lit.clone();
            out.lemma_type = Arc::clone(schema_type);
            Ok(out)
        }
        (TypeSpecification::Measure { .. }, ValueKind::Measure(_)) => {
            if !lit.lemma_type.same_measure_family(schema_ref)
                && !lit.lemma_type.compatible_with_anonymous_measure(schema_ref)
                && !lit
                    .lemma_type
                    .specifications
                    .equal_ignoring_unit(&schema_ref.specifications)
            {
                return Err(format!(
                    "value {} cannot be used as type {}: incompatible measure families",
                    lit,
                    schema_ref.name()
                ));
            }
            let mut out = lit.clone();
            out.lemma_type = preserve_measure_binding(schema_type, &lit.lemma_type);
            Ok(out)
        }
        (TypeSpecification::Ratio { .. }, ValueKind::Ratio(_)) => {
            let mut out = lit.clone();
            out.lemma_type = preserve_measure_binding(schema_type, &lit.lemma_type);
            Ok(out)
        }
        _ => Err(format!(
            "value {} cannot be used as type {}",
            lit,
            schema_ref.name()
        )),
    }
}

/// Named ranged types (`cargo_mass range`, `score range`) refresh from the element after
/// decomposition. Apply their constraints only after that refresh so units/decomp exist and
/// range-local bounds overwrite inherited endpoints as last writer.
fn should_defer_ranged_constraints(parent: &ParentType) -> bool {
    matches!(parent, ParentType::Ranged { .. })
        && matches!(
            element_parent_type(parent),
            ParentType::Custom { .. } | ParentType::Qualified { .. }
        )
}

/// Fold a list of definition-style constraints into a [`TypeSpecification`].
/// Used for both the GraphBuilder's regular TypeDeclaration path and the
/// post-build reference type-merging pass, so the underlying constraint
/// application logic stays in one place.
pub(crate) struct ConstraintSuggestionSink<'a> {
    pub(crate) suggestion: &'a mut Option<RawSuggestion>,
    pub(crate) value: &'a mut Option<RawSuggestion>,
    pub(crate) resolve_unit: Option<&'a UnitTokenResolver<'a>>,
}

pub(crate) fn apply_constraints_to_spec(
    spec: &LemmaSpec,
    type_name: &str,
    data_name: &str,
    lemma_type: &mut LemmaType,
    constraints: &[Constraint],
    source: &Source,
    sink: ConstraintSuggestionSink<'_>,
) -> Result<(), Vec<Error>> {
    let mut errors = Vec::new();

    let mut seen_unit_names: std::collections::HashSet<String> = Default::default();
    let mut seen_singleton_commands: std::collections::HashSet<TypeConstraintCommand> =
        Default::default();
    let mut has_suggest = false;
    let mut has_value = false;
    for row in constraints.iter() {
        let command = row.command;
        let args = &row.args;
        if command == TypeConstraintCommand::Suggest {
            has_suggest = true;
        }
        if command == TypeConstraintCommand::With {
            has_value = true;
        }
        if command == TypeConstraintCommand::Unit {
            if let Some(CommandArg::Label(name)) = args.first() {
                if !seen_unit_names.insert(name.clone()) {
                    errors.push(Error::validation_with_context(
                        format!(
                            "Duplicate unit '{}': each unit name may appear at most once per type definition",
                            name
                        ),
                        Some(source.clone()),
                        None::<String>,
                        Some(spec),
                        None,
                    ));
                }
            }
        }
        if matches!(
            command,
            TypeConstraintCommand::Minimum
                | TypeConstraintCommand::Maximum
                | TypeConstraintCommand::Decimals
                | TypeConstraintCommand::Suggest
                | TypeConstraintCommand::With
        ) && !(command == TypeConstraintCommand::With
            && matches!(lemma_type.specifications, TypeSpecification::List { .. }))
            && !seen_singleton_commands.insert(command)
        {
            errors.push(Error::validation_with_context(
                format!(
                    "Duplicate '{command}' constraint: each may appear at most once per type declaration"
                ),
                Some(source.clone()),
                None::<String>,
                Some(spec),
                None,
            ));
        }
    }
    if has_suggest && has_value {
        errors.push(Error::validation_with_context(
            "Cannot use both 'with' and 'suggest' on the same data declaration".to_string(),
            Some(source.clone()),
            None::<String>,
            Some(spec),
            None,
        ));
    }
    if !errors.is_empty() {
        return Err(errors);
    }

    let ConstraintSuggestionSink {
        suggestion: declared_suggestion,
        value: declared_value,
        resolve_unit,
    } = sink;
    let names = ConstraintNames {
        type_name,
        data_name,
    };

    let mut apply_one = |lemma_type: &mut LemmaType,
                         command: TypeConstraintCommand,
                         args: &[CommandArg],
                         declared_suggestion: &mut Option<RawSuggestion>,
                         declared_value: &mut Option<RawSuggestion>| {
        let mut suggestion_before = declared_suggestion.clone();
        let mut value_before = declared_value.clone();
        let applied = if matches!(
            command,
            TypeConstraintCommand::Option | TypeConstraintCommand::Options
        ) {
            apply_option_constraint(lemma_type, command, args, resolve_unit, spec)
        } else if matches!(
            command,
            TypeConstraintCommand::Suggest | TypeConstraintCommand::With
        ) && matches!(lemma_type.specifications, TypeSpecification::Spec { .. })
        {
            let target = match command {
                TypeConstraintCommand::Suggest => &mut suggestion_before,
                TypeConstraintCommand::With => &mut value_before,
                _ => unreachable!("BUG: only Suggest or With in this arm"),
            };
            apply_spec_member_command(lemma_type, command, args, spec, target)
        } else if matches!(lemma_type.specifications, TypeSpecification::List { .. })
            && command == TypeConstraintCommand::With
        {
            apply_list_with_rows(
                lemma_type,
                names,
                args,
                &mut value_before,
                resolve_unit,
                spec,
            )
        } else {
            lemma_type.apply_constraint(
                names,
                command,
                args,
                &mut suggestion_before,
                &mut value_before,
                resolve_unit,
            )
        };
        match applied {
            Ok(()) => {
                *declared_suggestion = suggestion_before;
                *declared_value = value_before;
            }
            Err(e) => {
                errors.push(Error::validation_with_context(
                    format!("Failed to apply constraint '{}': {}", command, e),
                    Some(source.clone()),
                    None::<String>,
                    Some(spec),
                    None,
                ));
            }
        }
    };

    for row in constraints {
        let command = row.command;
        let args = &row.args;
        if matches!(
            command,
            TypeConstraintCommand::Unit | TypeConstraintCommand::Trait
        ) {
            apply_one(
                lemma_type,
                command,
                args,
                declared_suggestion,
                declared_value,
            );
        }
    }
    for row in constraints {
        let command = row.command;
        let args = &row.args;
        if !matches!(
            command,
            TypeConstraintCommand::Unit | TypeConstraintCommand::Trait
        ) {
            apply_one(
                lemma_type,
                command,
                args,
                declared_suggestion,
                declared_value,
            );
        }
    }
    if !errors.is_empty() {
        return Err(errors);
    }
    Ok(())
}

type UnitDecompLookup = HashMap<String, (Option<String>, BaseMeasureVector, RationalInteger)>;

fn local_owner_name(owner: &PlanningOwner) -> Option<String> {
    match owner {
        PlanningOwner::Local { type_name } => Some(type_name.clone()),
        PlanningOwner::PrimitiveRatio | PlanningOwner::Imported { .. } => None,
    }
}

fn unwrap_unique_resolved_type(arc: Arc<LemmaType>) -> LemmaType {
    Arc::try_unwrap(arc).expect("BUG: resolved LemmaType Arc unique during type resolution")
}

fn declared_measure_decomposition(lemma_type: &LemmaType) -> BaseMeasureVector {
    match &lemma_type.specifications {
        TypeSpecification::Measure { traits, .. }
            if traits.contains(&semantics::MeasureTrait::Duration) =>
        {
            duration_decomposition()
        }
        TypeSpecification::Measure { traits, .. }
            if traits.contains(&semantics::MeasureTrait::Calendar) =>
        {
            calendar_decomposition()
        }
        _ => {
            let family = lemma_type
                .measure_family()
                .expect("BUG: named measure type must carry a family");
            [(semantics::Dimension::Family(family.clone()), 1i32)]
                .into_iter()
                .collect()
        }
    }
}

/// Local type owning `unit_name`; `None` when unknown or imported.
fn local_owning_measure_type_for_unit(
    unit_name: &str,
    lookup: &UnitDecompLookup,
    unit_index: &PlanningUnitIndex,
    resolved: &IndexMap<String, Arc<LemmaType>>,
) -> Option<String> {
    if let Some((owning_measure_name, _, _)) = lookup.get(unit_name) {
        return owning_measure_name.clone();
    }
    unit_index
        .resolve_owner(unit_name, resolved)
        .ok()
        .and_then(|(_, owner)| local_owner_name(owner))
}

/// Order compound measure types so every referenced unit from another compound type is resolved first.
fn sort_derived_measure_types_for_resolution(
    spec_name: &str,
    derived_measure_type_names: Vec<String>,
    resolved: &IndexMap<String, Arc<LemmaType>>,
    lookup: &UnitDecompLookup,
    unit_index: &PlanningUnitIndex,
    type_sources: &HashMap<String, Source>,
) -> Result<Vec<String>, Error> {
    let derived_measure_type_count = derived_measure_type_names.len();
    if derived_measure_type_count == 0 {
        return Ok(derived_measure_type_names);
    }

    let type_index: HashMap<&str, usize> = derived_measure_type_names
        .iter()
        .enumerate()
        .map(|(index, name)| (name.as_str(), index))
        .collect();

    let mut dependency_sets: Vec<BTreeSet<usize>> =
        vec![BTreeSet::new(); derived_measure_type_count];

    for (dependent_index, type_name) in derived_measure_type_names.iter().enumerate() {
        let TypeSpecification::Measure { units, .. } = &resolved[type_name].specifications else {
            continue;
        };
        for unit in units.iter() {
            for (factor_unit_name, _) in &unit.derived_measure_factors {
                let Some(owning_measure_name) = local_owning_measure_type_for_unit(
                    factor_unit_name,
                    lookup,
                    unit_index,
                    resolved,
                ) else {
                    continue;
                };
                let Some(dependency_index) = type_index.get(owning_measure_name.as_str()).copied()
                else {
                    continue;
                };
                if dependency_index == dependent_index {
                    continue;
                }
                dependency_sets[dependent_index].insert(dependency_index);
            }
        }
    }

    let mut in_degree = vec![0usize; derived_measure_type_count];
    let mut dependents: Vec<Vec<usize>> = vec![Vec::new(); derived_measure_type_count];

    for (dependent_index, dependencies) in dependency_sets.iter().enumerate() {
        for &dependency_index in dependencies {
            in_degree[dependent_index] += 1;
            dependents[dependency_index].push(dependent_index);
        }
    }

    let mut queue: VecDeque<usize> = (0..derived_measure_type_count)
        .filter(|&index| in_degree[index] == 0)
        .collect();
    let mut sorted_indices: Vec<usize> = Vec::with_capacity(derived_measure_type_count);

    while let Some(index) = queue.pop_front() {
        sorted_indices.push(index);
        for &dependent_index in &dependents[index] {
            in_degree[dependent_index] -= 1;
            if in_degree[dependent_index] == 0 {
                queue.push_back(dependent_index);
            }
        }
    }

    if sorted_indices.len() != derived_measure_type_count {
        let mut cycle_type_names: Vec<String> = (0..derived_measure_type_count)
            .filter(|&index| in_degree[index] > 0)
            .map(|index| derived_measure_type_names[index].clone())
            .collect();
        cycle_type_names.sort();
        return Err(Error::validation(
            format!(
                "In spec '{}': circular compound measure type dependency among: {}",
                spec_name,
                cycle_type_names.join(", ")
            ),
            type_sources.get(&cycle_type_names[0]).cloned(),
            None::<String>,
        ));
    }

    Ok(sorted_indices
        .into_iter()
        .map(|index| derived_measure_type_names[index].clone())
        .collect())
}

/// Build the reverse signature index over every named-measure-type unit in the spec.
/// Each unit's canonical-form `derived_measure_factors` becomes a key; the value is the
/// owning unit name and type. An ambiguity (the same canonical key matched by two units in
/// distinct types) is a planning error: the spec must rename one of the units or change
/// its factor decomposition so it is unique.
pub(crate) struct SignatureIndexBuild {
    pub index: SignatureIndex,
    pub deferred_cross_type: Vec<(String, BaseMeasureVector)>,
}

pub(crate) fn build_signature_index(
    spec_name: &str,
    unit_index: &UnitIndex,
    already_resolved: &ResolvedTypesMap<'_>,
) -> Result<SignatureIndexBuild, Error> {
    use crate::computation::arithmetic::SignatureIndex;
    let mut signature_index: SignatureIndex = SignatureIndex::new();
    let mut deferred_cross_type = Vec::new();
    let mut sorted_units: Vec<_> = unit_index.iter_owners().collect();
    sorted_units.sort_by_key(|(name, _)| *name);
    for (unit_name, owner) in sorted_units {
        let unit_name = unit_name.to_string();
        let lemma_type = owner.lemma_type();
        let TypeSpecification::Measure { units, .. } = &lemma_type.specifications else {
            continue;
        };
        let unit = units.get(unit_name.as_str()).map_err(|_| {
            Error::validation(
                format!(
                    "In spec '{}': unit_index entry '{}' is not declared on measure type '{}'",
                    spec_name,
                    unit_name,
                    lemma_type.name()
                ),
                None::<Source>,
                None::<String>,
            )
        })?;
        if lemma_type.measure_type_decomposition().is_none() {
            continue;
        }
        // Identity signatures `[(name, 1)]` collide when the same bare name has
        // multiple owners. Skip those; keep unique identities so expand can
        // promote compounds that reduce to a single unique unit (e.g. eur).
        if measure_unit_is_simple(unit) && unit_index.owners_for(&unit_name).len() != 1 {
            continue;
        }
        let owning_type_name = lemma_type.name();
        let signature = if owner.import_path().is_empty() {
            resolved_signature_key(
                &unit.derived_measure_factors,
                &unit_name,
                lemma_type,
                unit_index,
            )
        } else if let Some(key) = imported_signature_key(already_resolved, &unit_name, lemma_type) {
            key
        } else if measure_unit_is_simple(unit) {
            // Declaring spec skipped this identity because the name had several
            // owners there. Here it is unique, and the key is the unit's own family.
            resolved_signature_key(
                &unit.derived_measure_factors,
                &unit_name,
                lemma_type,
                unit_index,
            )
        } else {
            // Same type, same signature: the index keeps one slot, under the
            // sibling unit that was inserted last (`square_meter` for `acre`).
            continue;
        };
        match signature_index.get(&signature) {
            Some((existing_unit_name, existing_owning_type))
                if existing_owning_type.name() != lemma_type.name() =>
            {
                let existing_type_name = existing_owning_type.name();
                let decomposition = lemma_type
                    .measure_type_decomposition()
                    .expect("BUG: indexed measure unit has no decomposition")
                    .clone();
                deferred_cross_type.push((
                    format!(
                        "In spec '{spec_name}': ambiguous unit signature {signature:?}: matched by '{existing_unit_name}' in type '{existing_type_name}' and '{unit_name}' in type '{owning_type_name}'. \
                         Rename one or differentiate factors."
                    ),
                    decomposition,
                ));
            }
            _ => {
                signature_index.insert(signature, (unit_name.clone(), Arc::clone(lemma_type)));
            }
        }
    }
    Ok(SignatureIndexBuild {
        index: signature_index,
        deferred_cross_type,
    })
}

fn resolved_signature_key(
    factors: &[(String, i32)],
    unit_name: &str,
    lemma_type: &LemmaType,
    unit_index: &UnitIndex,
) -> Vec<SignatureFactor> {
    use crate::computation::arithmetic::SignatureFactor;
    let mut key = Vec::with_capacity(factors.len());
    for (factor_name, exponent) in factors {
        let (name, family) = if factor_name == unit_name {
            (
                factor_name.clone(),
                lemma_type
                    .measure_family()
                    .expect("BUG: indexed measure unit has no family")
                    .clone(),
            )
        } else {
            let (bare, owner) = unit_index.resolve_owner(factor_name).unwrap_or_else(|err| {
                panic!(
                    "BUG: compound factor '{factor_name}' on unit '{unit_name}' must resolve: {err}"
                )
            });
            (
                bare,
                owner
                    .lemma_type()
                    .measure_family()
                    .expect("BUG: resolved factor owner is not a named measure")
                    .clone(),
            )
        };
        key.push(SignatureFactor {
            name,
            family,
            exponent: *exponent,
        });
    }
    key.sort_by(|left, right| {
        (&left.family, left.name.as_str(), left.exponent).cmp(&(
            &right.family,
            right.name.as_str(),
            right.exponent,
        ))
    });
    key
}

/// Key already built for this unit by the spec that declared it.
/// The imported owner holds the same `LemmaType` allocation as that index entry.
fn imported_signature_key(
    already_resolved: &ResolvedTypesMap<'_>,
    unit_name: &str,
    lemma_type: &Arc<LemmaType>,
) -> Option<Vec<SignatureFactor>> {
    for (_, _, types) in already_resolved {
        for (key, (name, owner)) in &types.signature_index {
            if name == unit_name && Arc::ptr_eq(owner, lemma_type) {
                return Some(key.clone());
            }
        }
    }
    None
}

pub(crate) fn deferred_signature_errors(
    main_spec: &LemmaSpec,
    resolved_types: &ResolvedTypesMap,
    inferred: &HashMap<RulePath, Arc<LemmaType>>,
) -> Vec<Error> {
    let Some(types) = resolved_types
        .iter()
        .find_map(|(_, spec, types)| discovery::same_loaded_spec(spec, main_spec).then_some(types))
    else {
        return Vec::new();
    };
    let mut errors = Vec::new();
    for (message, decomposition) in &types.deferred_signature_collisions {
        let produced = inferred.values().any(|lemma_type| {
            lemma_type.is_anonymous_measure()
                && lemma_type.measure_type_decomposition() == Some(decomposition)
        });
        if produced {
            continue;
        }
        errors.push(Error::validation(
            message.clone(),
            None::<Source>,
            None::<String>,
        ));
    }
    errors
}

/// Rebuild [`MeasureRange`] (and other range) specs for `ParentType::Ranged` declarations
/// after the decomposition pass has populated parent measure types.
fn refresh_named_range_specs(
    resolver: &TypeResolver<'_>,
    spec: &LemmaSpec,
    data_defs: &HashMap<String, DataTypeDef>,
    resolved_types: &mut ResolvedSpecTypes,
    already_resolved: &ResolvedTypesMap<'_>,
    at: &EffectiveDate,
) -> Vec<Error> {
    let local = local_type_entries(resolved_types);
    let resolved = &mut resolved_types.resolved;
    let declared_suggestions = &mut resolved_types.declared_suggestions;
    let mut errors = Vec::new();
    for (type_name, def) in data_defs {
        let ParentType::Ranged { .. } = &def.parent else {
            continue;
        };
        let element = element_parent_type(&def.parent);
        if matches!(element, ParentType::Primitive { .. }) {
            continue;
        }
        let scope = ParentLookup {
            resolver,
            spec,
            source: &def.source,
            at,
            already_resolved,
            resolved: &local,
            site: TypeUseSite::DataDefinition,
        };
        let endpoint_type = match lookup_parent_type(&scope, element) {
            Ok((.., endpoint_type)) => endpoint_type,
            Err(lookup_errors) => {
                errors.extend(lookup_errors);
                continue;
            }
        };
        let range_spec = endpoint_type
            .specifications
            .range_from_element()
            .expect("BUG: ranged element was checked rangeable when the type was resolved");
        let lemma_type = resolved
            .get_mut(type_name.as_str())
            .expect("BUG: ranged type resolved before its range spec is refreshed");
        let mut updated = lemma_type.as_ref().clone();
        updated.specifications = range_spec;
        *lemma_type = Arc::new(updated);

        if let Some(BoundValueKind {
            value: ValueKind::Range(left, right),
            ..
        }) = declared_suggestions.get_mut(type_name.as_str())
        {
            let left_typed = left.as_ref().clone();
            let right_typed = right.as_ref().clone();
            let coerced_left = match coerce_literal_to_schema_type(&left_typed, &endpoint_type) {
                Ok(coerced) => coerced,
                Err(message) => {
                    errors.push(Error::validation_with_context(
                        format!(
                            "In spec '{}': named range default left endpoint for '{}': {message}",
                            spec.name, type_name
                        ),
                        Some(def.source.clone()),
                        None::<String>,
                        Some(spec),
                        None,
                    ));
                    continue;
                }
            };
            let coerced_right = match coerce_literal_to_schema_type(&right_typed, &endpoint_type) {
                Ok(coerced) => coerced,
                Err(message) => {
                    errors.push(Error::validation_with_context(
                        format!(
                            "In spec '{}': named range default right endpoint for '{}': {message}",
                            spec.name, type_name
                        ),
                        Some(def.source.clone()),
                        None::<String>,
                        Some(spec),
                        None,
                    ));
                    continue;
                }
            };
            *declared_suggestions
                .get_mut(type_name.as_str())
                .expect("BUG: named range default removed while refreshing endpoints") =
                BoundValueKind {
                    value: ValueKind::Range(Box::new(coerced_left), Box::new(coerced_right)),
                    unit: None,
                    instance_factors: None,
                };
        }
    }
    errors
}

/// Apply range-local constraints after [`refresh_named_range_specs`] for named ranged types.
fn apply_deferred_named_range_constraints(
    spec: &LemmaSpec,
    data_defs: &HashMap<String, DataTypeDef>,
    resolved: &mut TypeMap,
    declared_suggestions: &mut IndexMap<String, BoundValueKind>,
    declared_values: &mut IndexMap<String, BoundValueKind>,
    type_sources: &HashMap<String, Source>,
    unit_index: &PlanningUnitIndex,
) -> Vec<Error> {
    let mut errors = Vec::new();
    for (type_name, def) in data_defs {
        if !should_defer_ranged_constraints(&def.parent) {
            continue;
        }
        let Some(constraints) = &def.constraints else {
            continue;
        };
        let Some(lemma_type) = resolved.get(type_name.as_str()).cloned() else {
            continue;
        };
        let mut declared_suggestion: Option<RawSuggestion> = None;
        let mut declared_value: Option<RawSuggestion> = None;
        let resolve = UnitTokenResolver::Planning {
            unit_index,
            named_types: resolved,
        };
        let mut updated = lemma_type.as_ref().clone();
        match apply_constraints_to_spec(
            spec,
            &constraint_application_type_name(&def.parent, type_name),
            type_name,
            &mut updated,
            constraints,
            &def.source,
            ConstraintSuggestionSink {
                suggestion: &mut declared_suggestion,
                value: &mut declared_value,
                resolve_unit: Some(&resolve),
            },
        ) {
            Ok(()) => {
                let updated_arc = Arc::new(updated);
                if let Some(raw) = declared_suggestion {
                    match bound_value_kind_from_raw_suggestion(
                        raw,
                        &updated_arc.specifications,
                        type_name.as_str(),
                    ) {
                        Ok(value_kind) => {
                            declared_suggestions.insert(type_name.clone(), value_kind);
                        }
                        Err(message) => {
                            let source = type_sources
                                .get(type_name.as_str())
                                .cloned()
                                .unwrap_or_else(|| def.source.clone());
                            errors.push(Error::validation_with_context(
                                message,
                                Some(source),
                                None::<String>,
                                Some(spec),
                                None,
                            ));
                        }
                    }
                }
                if let Some(raw) = declared_value {
                    match bound_value_kind_from_raw_suggestion(
                        raw,
                        &updated_arc.specifications,
                        type_name.as_str(),
                    ) {
                        Ok(value_kind) => {
                            declared_values.insert(type_name.clone(), value_kind);
                        }
                        Err(message) => {
                            let source = type_sources
                                .get(type_name.as_str())
                                .cloned()
                                .unwrap_or_else(|| def.source.clone());
                            errors.push(Error::validation_with_context(
                                message,
                                Some(source),
                                None::<String>,
                                Some(spec),
                                None,
                            ));
                        }
                    }
                }
                resolved.insert(type_name.clone(), updated_arc);
            }
            Err(constraint_errors) => errors.extend(constraint_errors),
        }
    }
    errors
}

/// Where a type is written, for planning error wording.
#[derive(Clone, Copy)]
enum TypeUseSite {
    DataDefinition,
    RuleListType,
}

impl fmt::Display for TypeUseSite {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TypeUseSite::DataDefinition => write!(f, "data definition"),
            TypeUseSite::RuleListType => write!(f, "list type in a rule"),
        }
    }
}

/// Scope a parent type name resolves in: the spec's local types so far, the specs it
/// imports, and where the type is written.
#[derive(Clone, Copy)]
struct ParentLookup<'s, 'a> {
    resolver: &'s TypeResolver<'a>,
    spec: &'s LemmaSpec,
    source: &'s Source,
    at: &'s EffectiveDate,
    already_resolved: &'s ResolvedTypesMap<'a>,
    resolved: &'s HashMap<String, ResolvedTypeEntry>,
    site: TypeUseSite,
}

fn lookup_parent_type(
    scope: &ParentLookup<'_, '_>,
    parent: &ParentType,
) -> Result<ParentLookupOk, Vec<Error>> {
    let mut wrappers = Vec::new();
    let mut leaf = parent;
    while let ParentType::List { inner } | ParentType::Ranged { inner } = leaf {
        wrappers.push(leaf);
        leaf = inner.as_ref();
    }
    let mut lookup = lookup_leaf_parent_type(scope, leaf)?;
    while let Some(wrapper) = wrappers.pop() {
        lookup = lookup_wrapper_parent_type(scope, wrapper, lookup)?;
    }
    Ok(lookup)
}

/// Lookup of a `list` or `range` wrapper, given the lookup of its `inner`.
fn lookup_wrapper_parent_type(
    scope: &ParentLookup<'_, '_>,
    wrapper: &ParentType,
    inner_lookup: ParentLookupOk,
) -> Result<ParentLookupOk, Vec<Error>> {
    let ParentLookup { spec, source, .. } = *scope;
    let (element_specs, .., element_type) = inner_lookup;
    match wrapper {
        ParentType::List { inner } => {
            let element = if matches!(inner.as_ref(), ParentType::Ranged { .. }) {
                Arc::new(LemmaType::primitive(element_specs))
            } else {
                element_type
            };
            let lemma_type = Arc::new(LemmaType::list_of(element));
            Ok((lemma_type.specifications.clone(), None, None, lemma_type))
        }
        ParentType::Ranged { inner } => {
            let range_spec = element_specs.range_from_element().ok_or_else(|| {
                vec![Error::validation_with_context(
                    format!(
                        "'{inner}' is not rangeable: only measure, number, ratio, date, and time types support ranges"
                    ),
                    Some(source.clone()),
                    None::<String>,
                    Some(spec),
                    None,
                )]
            })?;
            Ok((range_spec, None, None, element_type))
        }
        ParentType::Primitive { .. } | ParentType::Custom { .. } | ParentType::Qualified { .. } => {
            unreachable!("BUG: lookup_wrapper_parent_type called on a leaf parent")
        }
    }
}

/// Lookup of a parent that is not a `list` or `range` wrapper.
fn lookup_leaf_parent_type(
    scope: &ParentLookup<'_, '_>,
    parent: &ParentType,
) -> Result<ParentLookupOk, Vec<Error>> {
    let ParentLookup {
        resolver,
        spec,
        source,
        at,
        already_resolved,
        resolved,
        site,
    } = *scope;
    match parent {
        ParentType::List { .. } | ParentType::Ranged { .. } => {
            unreachable!("BUG: list and range wrappers are unwound before the leaf lookup")
        }
        ParentType::Primitive { primitive: kind } => {
            let lemma_type = Arc::clone(semantics::lemma_type_for_primitive(*kind));
            Ok((
                lemma_type.as_ref().specifications.clone(),
                None,
                None,
                lemma_type,
            ))
        }
        ParentType::Custom { name, .. } => {
            if let Some((parent_type, parent_suggestion, parent_value)) =
                resolved.get(name.as_str())
            {
                return Ok((
                    parent_type.as_ref().specifications.clone(),
                    parent_suggestion.clone(),
                    parent_value.clone(),
                    Arc::clone(parent_type),
                ));
            }
            let type_exists = resolver
                .data_types
                .iter()
                .find(|(s, _)| discovery::same_loaded_spec(s, spec))
                .map(|(_, type_map)| type_map.contains_key(name.as_str()))
                .unwrap_or(false);
            if !type_exists {
                if spec.data.iter().any(|data| {
                    data.reference.is_local()
                        && data.reference.name == name.as_str()
                        && data.value.uses_spec_ref().is_some()
                }) {
                    return Err(vec![Error::validation_with_context(
                        format!(
                            "'{name}' names a spec import alias, not a type. Use `data x: {name}.TypeName` after `uses`"
                        ),
                        Some(source.clone()),
                        None::<String>,
                        Some(spec),
                        None,
                    )]);
                }
                if spec.rules.iter().any(|rule| rule.name == name.as_str()) {
                    return Err(vec![Error::validation_with_context(
                        format!(
                            "Unknown parent '{parent}' for {site}: '{name}' is a local rule, not a type. Use a type name, or a reference expression if you meant the rule's value."
                        ),
                        Some(source.clone()),
                        None::<String>,
                        Some(spec),
                        None,
                    )]);
                }
                let mut imported_aliases: Vec<&str> = Vec::new();
                for data_row in &spec.data {
                    let Some(spec_ref) = data_row.value.uses_spec_ref() else {
                        continue;
                    };
                    let import_alias = &data_row.reference.name;
                    let (_, imported_spec) =
                        match resolver.resolve_spec_for_import(spec, spec_ref, source, at) {
                            Ok(pair) => pair,
                            Err(_) => continue,
                        };
                    let Some(imported_resolved) = already_resolved
                        .iter()
                        .find(|(_, s, _)| discovery::same_loaded_spec(s, imported_spec))
                        .map(|(_, _, r)| r)
                    else {
                        continue;
                    };
                    if imported_resolved.resolved.contains_key(name.as_str()) {
                        imported_aliases.push(import_alias.as_str());
                    }
                }
                if !imported_aliases.is_empty() {
                    let forms = imported_aliases
                        .iter()
                        .map(|alias| format!("{alias}.{name}"))
                        .collect::<Vec<_>>()
                        .join(", ");
                    return Err(vec![Error::validation_with_context(
                        format!(
                            "Unknown parent '{parent}' for {site}: '{name}' is declared by import{}; write {forms}",
                            if imported_aliases.len() == 1 {
                                format!(" '{}'", imported_aliases[0])
                            } else {
                                format!("s {}", imported_aliases.join(", "))
                            }
                        ),
                        Some(source.clone()),
                        None::<String>,
                        Some(spec),
                        None,
                    )]);
                }
                if resolver
                    .context
                    .spec_set(resolver.registered_repository(spec), name)
                    .is_some()
                {
                    return Err(vec![Error::validation_with_context(
                        format!(
                            "Unknown parent '{parent}' for {site}: '{name}' is a spec, not a name in spec '{}'. Only the local names of `uses` lines exist inside a spec",
                            spec.name
                        ),
                        Some(source.clone()),
                        Some(format!(
                            "Add `uses {name}`, or write the local name of the `uses` line that imports '{name}'."
                        )),
                        Some(spec),
                        None,
                    )]);
                }
                return Err(vec![Error::validation_with_context(
                    format!(
                        "Unknown parent '{parent}' for {site}. Parent must be defined before use. Valid primitive types are: boolean, measure, number, ratio, text, date, time"
                    ),
                    Some(source.clone()),
                    None::<String>,
                    Some(spec),
                    None,
                )]);
            }
            Err(vec![Error::validation_with_context(
                format!(
                    "Circular dependency detected in type resolution: {}::{}",
                    spec.name, name
                ),
                Some(source.clone()),
                None::<String>,
                Some(spec),
                None,
            )])
        }
        ParentType::Qualified { spec_alias, inner } => {
            let import_row = spec
                .data
                .iter()
                .find(|row| row.reference.segments.is_empty() && row.reference.name == *spec_alias);
            let spec_ref = import_row.and_then(|row| {
                row.value
                    .uses_spec_ref()
                    .cloned()
                    .or_else(|| discovery::data_instance_list_spec_ref(spec, row))
            });
            let Some(spec_ref) = spec_ref else {
                if spec.rules.iter().any(|rule| rule.name == *spec_alias) {
                    return Err(vec![Error::validation_with_context(
                        format!(
                            "'{spec_alias}' is a local rule, not a type path. Row fields in type position name a `uses` import or a `data` instance list, not a rule."
                        ),
                        Some(source.clone()),
                        None::<String>,
                        Some(spec),
                        None,
                    )]);
                }
                return Err(vec![Error::validation_with_context(
                    format!("'{spec_alias}' is not a `uses` import alias in this spec"),
                    Some(source.clone()),
                    Some(format!(
                        "Add `uses {spec_alias}: <target_spec>`, then write `{spec_alias}.<type>`."
                    )),
                    Some(spec),
                    None,
                )]);
            };
            let dynamic_effective = matches!(
                import_row.map(|row| &row.value),
                Some(ParsedDataValue::Import {
                    effective_ref: Some(_),
                    ..
                })
            );
            match inner.as_ref() {
                ParentType::Primitive { primitive: kind } => {
                    let lemma_type = Arc::clone(semantics::lemma_type_for_primitive(*kind));
                    Ok((
                        lemma_type.as_ref().specifications.clone(),
                        None,
                        None,
                        lemma_type,
                    ))
                }
                ParentType::Custom { name, .. } if dynamic_effective => {
                    union_dynamic_import_parent_type(
                        resolver,
                        spec,
                        source,
                        already_resolved,
                        &spec_ref,
                        spec_alias,
                        name.as_str(),
                    )
                }
                ParentType::Custom { name, .. } => {
                    let (import_repository, target_spec) =
                        match resolver.resolve_spec_for_import(spec, &spec_ref, source, at) {
                            Ok(import_pair) => import_pair,
                            Err(error) => return Err(vec![error]),
                        };
                    let Some(resolved_spec_types) = already_resolved
                        .iter()
                        .find(|(_, imported, _)| discovery::same_loaded_spec(imported, target_spec))
                        .map(|(_, _, resolved_spec_types)| resolved_spec_types)
                    else {
                        return Err(vec![Error::validation_with_context(
                            format!(
                                "Cannot resolve type '{name}' from spec '{}' (via import '{spec_alias}'): spec '{}' failed type resolution",
                                target_spec.name, target_spec.name
                            ),
                            Some(source.clone()),
                            None::<String>,
                            Some(spec),
                            None,
                        )]);
                    };
                    let Some(parent_type) = resolved_spec_types.resolved.get(name.as_str()) else {
                        let type_exists = resolver
                            .data_types
                            .iter()
                            .find(|(s, _)| discovery::same_loaded_spec(s, target_spec))
                            .map(|(_, type_map)| type_map.contains_key(name.as_str()))
                            .unwrap_or(false);
                        if !type_exists {
                            return Err(vec![Error::validation_with_context(
                                format!(
                                    "Type '{name}' is not defined in spec '{}' (via import '{spec_alias}')",
                                    target_spec.name
                                ),
                                Some(source.clone()),
                                None::<String>,
                                Some(spec),
                                None,
                            )]);
                        }
                        return Err(vec![Error::validation_with_context(
                            format!(
                                "Circular dependency detected in type resolution: {}::{}",
                                target_spec.name, name
                            ),
                            Some(source.clone()),
                            None::<String>,
                            Some(spec),
                            None,
                        )]);
                    };
                    let import_effective = spec_ref.at(at);
                    let identity =
                        SpecIdentity::of(&import_repository, target_spec, import_effective);
                    let hop = PathSegment::import(spec_alias, &identity);
                    let parent_type = Arc::new(parent_type.with_scale_import_hop(&hop));
                    Ok((
                        parent_type.as_ref().specifications.clone(),
                        resolved_spec_types
                            .source_defaults
                            .get(name.as_str())
                            .cloned(),
                        resolved_spec_types
                            .source_value_defaults
                            .get(name.as_str())
                            .cloned(),
                        parent_type,
                    ))
                }
                ParentType::Qualified { .. } => Err(vec![Error::validation_with_context(
                    "Nested qualified parent types are invalid",
                    Some(source.clone()),
                    None::<String>,
                    Some(spec),
                    None,
                )]),
                ParentType::Ranged { .. } => Err(vec![Error::validation_with_context(
                    "Nested ranged parent types are invalid",
                    Some(source.clone()),
                    None::<String>,
                    Some(spec),
                    None,
                )]),
                ParentType::List { .. } => Err(vec![Error::validation_with_context(
                    "Nested list parent types are invalid",
                    Some(source.clone()),
                    None::<String>,
                    Some(spec),
                    None,
                )]),
            }
        }
    }
}

/// Shared interface type for `data x: alias.Type` when `alias` is a `uses … -> effective`
/// import: options values are the sorted union across every candidate body; `options.rule`
/// must be absent on every body or identical.
fn union_dynamic_import_parent_type(
    resolver: &TypeResolver<'_>,
    spec: &LemmaSpec,
    source: &Source,
    already_resolved: &ResolvedTypesMap<'_>,
    spec_ref: &ast::SpecRef,
    spec_alias: &str,
    type_name: &str,
) -> Result<ParentLookupOk, Vec<Error>> {
    let resolved_repo = match discovery::resolve_spec_ref_repository(
        resolver.context,
        spec_ref,
        Some(source),
        spec.name.as_str(),
        Some(spec),
    ) {
        Ok(resolved) => resolved,
        Err(error) => return Err(vec![error]),
    };
    let repository = match &resolved_repo.repository {
        Some(explicit) => Arc::clone(explicit),
        None => Arc::clone(resolver.registered_repository(spec)),
    };
    let dep_name = crate::parsing::ast::ascii_lowercase_logical_name(spec_ref.name.clone());
    let mut candidates: Vec<&Arc<LemmaType>> = Vec::new();
    for (repo, imported, resolved_types) in already_resolved {
        if !Arc::ptr_eq(repo, &repository) {
            continue;
        }
        if crate::parsing::ast::ascii_lowercase_logical_name(imported.name.clone()) != dep_name {
            continue;
        }
        let Some(parent_type) = resolved_types.resolved.get(type_name) else {
            let type_exists = resolver
                .data_types
                .iter()
                .find(|(s, _)| discovery::same_loaded_spec(s, imported))
                .map(|(_, type_map)| type_map.contains_key(type_name))
                .unwrap_or(false);
            if !type_exists {
                return Err(vec![Error::validation_with_context(
                    format!(
                        "Type '{type_name}' is not defined in spec '{}' (via import '{spec_alias}')",
                        imported.name
                    ),
                    Some(source.clone()),
                    None::<String>,
                    Some(spec),
                    None,
                )]);
            }
            return Err(vec![Error::validation_with_context(
                format!(
                    "Circular dependency detected in type resolution: {}::{type_name}",
                    imported.name
                ),
                Some(source.clone()),
                None::<String>,
                Some(spec),
                None,
            )]);
        };
        candidates.push(parent_type);
    }
    if candidates.is_empty() {
        return Err(vec![Error::validation_with_context(
            format!(
                "Cannot resolve type '{type_name}' from dependency '{}' (via import '{spec_alias}'): no candidate body has completed type resolution",
                spec_ref.name
            ),
            Some(source.clone()),
            None::<String>,
            Some(spec),
            None,
        )]);
    }
    let first = candidates[0];
    for later in &candidates[1..] {
        if !first.same_interface_ignoring_options(later) {
            return Err(vec![Error::validation_with_context(
                format!(
                    "'{spec_alias}.{type_name}' changes across versions of '{}' under `-> effective`",
                    spec_ref.name
                ),
                Some(source.clone()),
                Some(format!(
                    "Keep '{type_name}' interface-compatible across every temporal version of '{}', or pin the import to a literal date.",
                    spec_ref.name
                )),
                Some(spec),
                None,
            )]);
        }
        if first.options.rule != later.options.rule {
            return Err(vec![Error::validation_with_context(
                format!(
                    "'{spec_alias}.{type_name}' has different `-> options` rules across versions of '{}' under `-> effective`",
                    spec_ref.name
                ),
                Some(source.clone()),
                Some(
                    "Use the same options rule name on every version, or only inline `-> option` values."
                        .to_string(),
                ),
                Some(spec),
                None,
            )]);
        }
    }
    let mut unioned = first.as_ref().clone();
    let mut values: Vec<semantics::OptionValue> = Vec::new();
    for candidate in &candidates {
        for option in &candidate.options.values {
            if !values.iter().any(|seen| seen == option) {
                values.push(option.clone());
            }
        }
    }
    values.sort_by(|left, right| {
        left.value
            .value
            .to_string()
            .cmp(&right.value.value.to_string())
            .then_with(|| left.value.unit.cmp(&right.value.unit))
    });
    unioned.options.values = values;
    let first_imported = already_resolved
        .iter()
        .find(|(repo, imported, _)| {
            Arc::ptr_eq(repo, &repository)
                && crate::parsing::ast::ascii_lowercase_logical_name(imported.name.clone())
                    == dep_name
        })
        .map(|(_, imported, _)| *imported)
        .expect("BUG: union candidates require a resolved import body");
    let identity = SpecIdentity::of(
        &repository,
        first_imported,
        first_imported.effective_from.clone(),
    );
    let hop = PathSegment::import(spec_alias, &identity);
    let unioned = unioned.with_scale_import_hop(&hop);
    Ok((
        unioned.specifications.clone(),
        None,
        None,
        Arc::new(unioned),
    ))
}

/// Order `type_names` so each type follows its local element parent. A cycle is a planning Error.
fn topo_sort_type_names(
    spec: &LemmaSpec,
    data_defs: &HashMap<String, DataTypeDef>,
    mut type_names: Vec<String>,
) -> Result<Vec<String>, Vec<Error>> {
    type_names.sort();
    if type_names.is_empty() {
        return Ok(type_names);
    }
    let type_index: HashMap<&str, usize> = type_names
        .iter()
        .enumerate()
        .map(|(index, name)| (name.as_str(), index))
        .collect();
    let type_count = type_names.len();
    let mut in_degree = vec![0usize; type_count];
    let mut dependents: Vec<Vec<usize>> = vec![Vec::new(); type_count];
    for (child_index, type_name) in type_names.iter().enumerate() {
        let custom_parent = match element_parent_type(&data_defs[type_name].parent) {
            ParentType::Custom { name, .. } => Some(name.as_str()),
            _ => None,
        };
        let Some(parent_name) = custom_parent else {
            continue;
        };
        let Some(&parent_index) = type_index.get(parent_name) else {
            continue;
        };
        if parent_index == child_index {
            continue;
        }
        in_degree[child_index] += 1;
        dependents[parent_index].push(child_index);
    }
    let mut queue: std::collections::VecDeque<usize> = (0..type_count)
        .filter(|&index| in_degree[index] == 0)
        .collect();
    let mut sorted = Vec::with_capacity(type_count);
    while let Some(index) = queue.pop_front() {
        sorted.push(type_names[index].clone());
        for &dependent_index in &dependents[index] {
            in_degree[dependent_index] -= 1;
            if in_degree[dependent_index] == 0 {
                queue.push_back(dependent_index);
            }
        }
    }
    if sorted.len() != type_count {
        let cycle_type_name = type_names
            .iter()
            .find(|name| in_degree[type_index[name.as_str()]] > 0)
            .expect("BUG: incomplete topo sort without cycle participant");
        return Err(vec![Error::validation_with_context(
            format!(
                "Circular dependency detected in type resolution: {}::{}",
                spec.name, cycle_type_name
            ),
            Some(data_defs[cycle_type_name].source.clone()),
            None::<String>,
            Some(spec),
            None,
        )]);
    }
    Ok(sorted)
}

/// Split local type declarations into scalar types and list types.
///
/// A list type (`money list`, or a type extending one) is resolved after every scalar
/// pass has finished, so its element is the final element type.
fn partition_list_type_names(
    data_defs: &HashMap<String, DataTypeDef>,
) -> (Vec<String>, Vec<String>) {
    fn has_list(parent: &ParentType) -> bool {
        let mut current = parent;
        loop {
            match current {
                ParentType::List { .. } => return true,
                ParentType::Ranged { inner } | ParentType::Qualified { inner, .. } => {
                    current = inner.as_ref();
                }
                ParentType::Primitive { .. } | ParentType::Custom { .. } => return false,
            }
        }
    }
    let mut list_names: HashSet<String> = data_defs
        .iter()
        .filter(|(_, def)| has_list(&def.parent))
        .map(|(name, _)| name.clone())
        .collect();
    loop {
        let before = list_names.len();
        for (name, def) in data_defs {
            if let ParentType::Custom { name: parent, .. } = element_parent_type(&def.parent) {
                if list_names.contains(parent) {
                    list_names.insert(name.clone());
                }
            }
        }
        if list_names.len() == before {
            break;
        }
    }
    let scalar_names = data_defs
        .keys()
        .filter(|name| !list_names.contains(*name))
        .cloned()
        .collect();
    (scalar_names, list_names.into_iter().collect())
}

type TypeMap = IndexMap<String, Arc<LemmaType>>;

/// Whether `name` is a local data field or rule in `spec`.
fn local_data_or_rule(spec: &LemmaSpec, name: &str) -> bool {
    let is_data = spec
        .data
        .iter()
        .any(|d| d.reference.is_local() && d.reference.name == name);
    let is_rule = spec.rules.iter().any(|r| r.name == name);
    is_data || is_rule
}

/// Whether `alias` is a `uses` import in `spec`.
fn is_uses_alias(spec: &LemmaSpec, alias: &str) -> bool {
    spec.data.iter().any(|d| {
        d.reference.is_local()
            && d.reference.name == alias
            && matches!(d.value, ParsedDataValue::Import { .. })
    })
}

/// Classify a `-> unit <name>: <reference>` payload: scale (data/rule) vs compound
/// (unit path). Lookup is against local data/rules, the unit index, and uses aliases —
/// never against hop count alone. A name that is both a local data/rule and a
/// resolvable unit path is ambiguous and must Error.
fn classify_unit_payload_reference(
    reference: &ast::Reference,
    spec: &LemmaSpec,
    unit_index: &PlanningUnitIndex,
    resolved: &TypeMap,
) -> Result<UnitPayloadClass, String> {
    let local = reference.segments.is_empty() && local_data_or_rule(spec, &reference.name);
    let unit_ref = reference.to_string();
    let resolves_as_unit = unit_index
        .resolve_with_named_types(&unit_ref, resolved)
        .is_ok();
    if local && resolves_as_unit {
        return Err(format!(
            "unit scale reference '{reference}' is ambiguous: it names both a local data or rule and a unit path"
        ));
    }
    if local {
        return Ok(UnitPayloadClass::Scale);
    }
    if resolves_as_unit {
        return Ok(UnitPayloadClass::Compound);
    }
    if matches!(reference.segments.as_slice(), [alias] if is_uses_alias(spec, alias)) {
        return Ok(UnitPayloadClass::Scale);
    }
    Err(format!(
        "Unknown unit '{unit_ref}'. Declare it on a measure or ratio type, or import it with uses."
    ))
}

enum UnitPayloadClass {
    Scale,
    Compound,
}

/// Classify still-unresolved unit payloads on units **this typedef owns**, and resolve
/// scale payloads to `Reference` targets in the coordinates of this spec: a local data
/// or rule, or one `uses` hop plus a member of the imported spec. Unit-path payloads
/// become compound `Literal(1)` factors. Returns planning errors.
fn rewrite_unit_path_scale_payloads(
    resolver: &TypeResolver<'_>,
    spec: &LemmaSpec,
    at: &EffectiveDate,
    already_resolved: &ResolvedTypesMap<'_>,
    resolved: &mut TypeMap,
    unit_index: &PlanningUnitIndex,
    type_sources: &HashMap<String, Source>,
) -> Vec<Error> {
    let mut errors = Vec::new();
    let type_names: Vec<String> = resolved.keys().cloned().collect();
    for type_name in type_names {
        let lemma_type = resolved
            .get(&type_name)
            .expect("BUG: type_name comes from resolved keys");
        let TypeSpecification::Measure { units, .. } = &lemma_type.specifications else {
            continue;
        };
        let unit_owned_locally = |unit_name: &str| {
            unit_index.owners_for(unit_name).iter().any(|owner| {
                matches!(
                    owner,
                    PlanningOwner::Local {
                        type_name: local_name
                    } if local_name == type_name.as_str()
                )
            })
        };
        let source = type_sources.get(&type_name).cloned();
        let mut compounds: Vec<(usize, String)> = Vec::new();
        let mut scale_targets: Vec<(usize, ReferenceTarget)> = Vec::new();
        for (idx, unit) in units.iter().enumerate() {
            let Some((segments, name)) = unit.scale_reference_parts() else {
                continue;
            };
            if unit.resolved_scale_target().is_some() {
                continue;
            }
            let reference = ast::Reference {
                segments: segments.to_vec(),
                name: name.to_string(),
            };
            let source = source.clone().unwrap_or_else(|| {
                panic!("BUG: measure type '{type_name}' with a unit payload has no type source")
            });
            // Inherited units were classified by the declaring type. Same-spec
            // extensions still hold Unresolved until this pass resolves them.
            if !unit_owned_locally(&unit.name) {
                match resolve_number_scale_reference(
                    resolver,
                    spec,
                    at,
                    already_resolved,
                    &reference,
                    &source,
                ) {
                    Ok(target) => scale_targets.push((idx, target)),
                    Err(error) => errors.push(error),
                }
                continue;
            }
            match classify_unit_payload_reference(&reference, spec, unit_index, resolved) {
                Ok(UnitPayloadClass::Scale) => {
                    match resolve_number_scale_reference(
                        resolver,
                        spec,
                        at,
                        already_resolved,
                        &reference,
                        &source,
                    ) {
                        Ok(target) => scale_targets.push((idx, target)),
                        Err(error) => errors.push(error),
                    }
                }
                Ok(UnitPayloadClass::Compound) => {
                    compounds.push((idx, reference.to_string()));
                }
                Err(message) => {
                    errors.push(Error::validation(
                        format!(
                            "In spec '{}': unit '{}' in measure type '{}': {}",
                            spec.name, unit.name, type_name, message
                        ),
                        Some(source),
                        None::<String>,
                    ));
                }
            }
        }
        if compounds.is_empty() && scale_targets.is_empty() {
            continue;
        }
        let mut owned = lemma_type.as_ref().clone();
        let TypeSpecification::Measure { units, .. } = &mut owned.specifications else {
            unreachable!("BUG: measure type still a measure after clone");
        };
        for (idx, target) in scale_targets {
            units.0[idx] = units.0[idx].clone().with_resolved_scale(target);
        }
        for (idx, referenced) in compounds {
            let name = units.0[idx].name.clone();
            units.0[idx] =
                MeasureUnit::from_decimal_factor(name, Decimal::ONE, vec![(referenced, 1)])
                    .expect("BUG: factor 1 always converts to a rational");
        }
        resolved.insert(type_name, Arc::new(owned));
    }
    errors
}

/// Resolved target of a `-> unit <name>: <reference>` payload classified as a number
/// scale, in the coordinates of `spec`: a local data or rule, or one `uses` hop plus a
/// member of the imported spec.
fn resolve_number_scale_reference(
    resolver: &TypeResolver<'_>,
    spec: &LemmaSpec,
    at: &EffectiveDate,
    already_resolved: &ResolvedTypesMap<'_>,
    reference: &ast::Reference,
    source: &Source,
) -> Result<ReferenceTarget, Error> {
    let scale_reference_error = |message: String| {
        Error::validation_with_context(
            message,
            Some(source.clone()),
            None::<String>,
            Some(spec),
            None,
        )
    };
    if reference.segments.is_empty() {
        let is_data = spec
            .data
            .iter()
            .any(|d| d.reference.is_local() && d.reference.name == reference.name);
        let is_rule = spec.rules.iter().any(|r| r.name == reference.name);
        if is_data && is_rule {
            return Err(scale_reference_error(format!(
                "Reference target '{}' is ambiguous: both a data and a rule in spec '{}'",
                reference.name, spec.name
            )));
        }
        if is_data {
            let is_import = spec.data.iter().any(|d| {
                d.reference.is_local()
                    && d.reference.name == reference.name
                    && matches!(d.value, ParsedDataValue::Import { .. })
            });
            if is_import {
                return Err(scale_reference_error(format!(
                    "cannot supply spec instance '{}' as a value",
                    reference.name
                )));
            }
            return Ok(ReferenceTarget::Data(DataPath::local(
                reference.name.clone(),
            )));
        }
        if is_rule {
            return Ok(ReferenceTarget::Rule(RulePath::new(
                Vec::new(),
                reference.name.clone(),
            )));
        }
        return Err(scale_reference_error(format!(
            "Reference target '{}' not found in spec '{}'",
            reference.name, spec.name
        )));
    }
    let [alias] = reference.segments.as_slice() else {
        return Err(scale_reference_error(format!(
            "unit scale reference '{reference}' is not a data or rule path in spec '{}'",
            spec.name
        )));
    };
    let Some(import_row) = spec.data.iter().find(|d| {
        d.reference.is_local()
            && d.reference.name == *alias
            && matches!(d.value, ParsedDataValue::Import { .. })
    }) else {
        return Err(scale_reference_error(format!(
            "Reference target '{}' not found in spec '{}'",
            reference, spec.name
        )));
    };
    let Some(spec_ref) = import_row.value.uses_spec_ref() else {
        return Err(scale_reference_error(format!(
            "Reference target '{}' not found in spec '{}'",
            reference, spec.name
        )));
    };
    let (import_repository, target_spec) =
        resolver.resolve_spec_for_import(spec, spec_ref, source, at)?;
    let Some((_, _, _)) = already_resolved
        .iter()
        .find(|(_, imported, _)| discovery::same_loaded_spec(imported, target_spec))
    else {
        return Err(scale_reference_error(format!(
            "Reference target '{}' not found in spec '{}'",
            reference.name, target_spec.name
        )));
    };
    let is_data = target_spec
        .data
        .iter()
        .any(|d| d.reference.is_local() && d.reference.name == reference.name);
    let is_rule = target_spec.rules.iter().any(|r| r.name == reference.name);
    if is_data && is_rule {
        return Err(scale_reference_error(format!(
            "Reference target '{}' is ambiguous: both a data and a rule in spec '{}'",
            reference.name, target_spec.name
        )));
    }
    let import_effective = spec_ref.at(at);
    let identity = SpecIdentity::of(&import_repository, target_spec, import_effective);
    let hop = PathSegment::import(alias, &identity);
    if is_data {
        return Ok(ReferenceTarget::Data(DataPath {
            segments: vec![hop],
            data: reference.name.clone(),
        }));
    }
    if is_rule {
        return Ok(ReferenceTarget::Rule(RulePath {
            segments: vec![hop],
            rule: reference.name.clone(),
        }));
    }
    Err(scale_reference_error(format!(
        "Reference target '{}' not found in spec '{}'",
        reference.name, target_spec.name
    )))
}

fn resolve_measure_decompositions(
    spec_name: &str,
    this_spec: &SpecVersionKey,
    mut resolved: TypeMap,
    unit_index: &PlanningUnitIndex,
    type_sources: &HashMap<String, Source>,
) -> (TypeMap, Vec<Error>) {
    let mut errors: Vec<Error> = Vec::new();

    let source_for = |type_name: &str| -> Option<Source> { type_sources.get(type_name).cloned() };

    let base_type_names: Vec<String> = resolved
        .iter()
        .filter_map(|(name, lt)| {
            if let TypeSpecification::Measure { units, .. } = &lt.specifications {
                if units.iter().all(measure_unit_is_simple) {
                    return Some(name.clone());
                }
            }
            None
        })
        .collect();

    for type_name in &base_type_names {
        let owned = unwrap_unique_resolved_type(
            resolved
                .shift_remove(type_name)
                .expect("BUG: type_name comes from resolved's own keys"),
        );
        let base_decomp = declared_measure_decomposition(&owned);

        let reserved_collision = reserved_calendar_units_in_measure(&owned);
        if !reserved_collision.is_empty() && !owned.has_trait_calendar() {
            errors.push(Error::validation(
                format!(
                    "In spec '{}': measure type '{}' declares unit(s) {:?} which are reserved calendar unit names.",
                    spec_name, type_name, reserved_collision
                ),
                source_for(type_name),
                None::<String>,
            ));
            resolved.insert(type_name.clone(), Arc::new(owned));
            continue;
        }

        let updated = owned.map_measure(|units, _| {
            let units = units.map(|u| u.with_decomposition(base_decomp.clone()));
            (units, Some(base_decomp.clone()))
        });
        resolved.insert(type_name.clone(), Arc::new(updated));
    }

    for lemma_type in resolved.values_mut() {
        let TypeSpecification::MeasureRange { units, .. } = &lemma_type.specifications else {
            continue;
        };
        if units.is_empty() {
            continue;
        }
        let family = lemma_type
            .family()
            .expect("BUG: measure range declaring units must be a named custom type");
        let range_decomp: BaseMeasureVector =
            [(semantics::Dimension::Family(family.clone()), 1i32)]
                .into_iter()
                .collect();
        let TypeSpecification::MeasureRange {
            units,
            decomposition,
            ..
        } = &mut Arc::get_mut(lemma_type)
            .expect("BUG: resolved LemmaType Arc unique during type resolution")
            .specifications
        else {
            unreachable!("BUG: matched MeasureRange above");
        };
        *units = std::mem::take(units).map(|u| u.with_decomposition(range_decomp.clone()));
        *decomposition = Some(range_decomp);
    }

    let mut lookup = UnitDecompLookup::new();

    for (unit_name, owner) in unit_index.iter_owners() {
        let unit_name = unit_name.to_string();
        let lemma_type = owner.lemma_type(&resolved);
        if let TypeSpecification::Measure {
            decomposition: Some(decomposition),
            units,
            ..
        } = &lemma_type.specifications
        {
            let measure_name = local_owner_name(owner);
            let unit = units
                .iter()
                .find(|u| u.name == unit_name)
                .unwrap_or_else(|| {
                    panic!(
                        "BUG: unit_name '{}' from unit_index must be in its owning type's units",
                        unit_name
                    )
                });
            let Some(factor) = unit.literal_factor().cloned() else {
                continue;
            };
            // Unique bare names only in decomp lookup; ambiguous factors resolve via UnitIndex.
            if unit_index.has_unique_owner(unit_name.as_str()) {
                lookup.insert(
                    unit_name.clone(),
                    (measure_name, decomposition.clone(), factor),
                );
            }
        }
    }

    for (type_name, lemma_type) in resolved.iter() {
        if let TypeSpecification::Measure {
            units,
            decomposition: Some(decomposition),
            ..
        } = &lemma_type.specifications
        {
            let is_defining_type = lemma_type.measure_family().is_some_and(|family| {
                family.is_declared_in(this_spec) && family.name == *type_name
            });
            if !is_defining_type {
                continue;
            }
            for unit in units.iter() {
                let declared_by_other_family =
                    unit_index.owners_for(&unit.name).iter().any(|owner| {
                        !owner
                            .lemma_type(&resolved)
                            .same_measure_family(lemma_type.as_ref())
                    });
                if declared_by_other_family {
                    continue;
                }
                let Some(factor) = unit.literal_factor().cloned() else {
                    continue;
                };
                lookup.insert(
                    unit.name.clone(),
                    (Some(type_name.clone()), decomposition.clone(), factor),
                );
            }
        }
    }

    let derived_measure_type_names_unsorted: Vec<String> = resolved
        .iter()
        .filter_map(|(name, lemma_type)| {
            if let TypeSpecification::Measure { units, .. } = &lemma_type.specifications {
                if units.iter().any(|unit| !measure_unit_is_simple(unit)) {
                    return Some(name.clone());
                }
            }
            None
        })
        .collect();

    let derived_measure_type_names = match sort_derived_measure_types_for_resolution(
        spec_name,
        derived_measure_type_names_unsorted,
        &resolved,
        &lookup,
        unit_index,
        type_sources,
    ) {
        Ok(sorted) => sorted,
        Err(error) => {
            errors.push(error);
            return (resolved, errors);
        }
    };

    for type_name in &derived_measure_type_names {
        let type_source = source_for(type_name);

        let (units_snapshot, already_resolved_body) = match &resolved[type_name].specifications {
            TypeSpecification::Measure {
                units,
                decomposition,
                ..
            } => (units.clone(), decomposition.clone()),
            _ => continue,
        };

        // Post-resolve import clones already carry `decomposition: Some`. Re-running
        // `resolve_compound_unit` on inherited units squares non-1 scales (eur/hour → 1/3600²).
        // Pure aliases (no locally owned units) leave the clone intact before the unit walk.
        let body_already_resolved = already_resolved_body.is_some();
        let unit_owned_locally = |unit_name: &str| {
            unit_index.owners_for(unit_name).iter().any(|owner| {
                matches!(
                    owner,
                    PlanningOwner::Local {
                        type_name: local_name
                    } if local_name == type_name
                )
            })
        };
        if body_already_resolved && !units_snapshot.iter().any(|u| unit_owned_locally(&u.name)) {
            for unit in units_snapshot.iter() {
                if matches!(&unit.scale, crate::literals::UnitScale::Unresolved { .. }) {
                    panic!(
                        "BUG: inherited unit '{}' on '{}' still has Unresolved scale; declaring family must classify and resolve unit payloads before dependents clone",
                        unit.name, type_name
                    );
                }
            }
            continue;
        }

        let mut resolved_type_decomp: Option<BaseMeasureVector> = already_resolved_body;
        let mut unit_errors: Vec<Error> = Vec::new();
        let mut resolved_unit_scales: Vec<Option<ResolvedCompoundScale>> =
            vec![None; units_snapshot.len()];
        let mut resolve_to_simple: Vec<bool> = vec![false; units_snapshot.len()];

        for (unit_idx, unit) in units_snapshot.iter().enumerate() {
            if calendar_unit_factor(&unit.name).is_some()
                && !resolved[type_name].has_trait_calendar()
            {
                unit_errors.push(Error::validation(
                    format!(
                        "In spec '{}': measure type '{}' declares unit '{}' which is a reserved calendar unit name.",
                        spec_name, type_name, unit.name
                    ),
                    type_source.clone(),
                    None::<String>,
                ));
                continue;
            }
            if measure_unit_is_simple(unit) {
                let simple_decomp = declared_measure_decomposition(&resolved[type_name]);

                if let Some(existing) = &resolved_type_decomp {
                    if existing != &simple_decomp {
                        unit_errors.push(Error::validation(
                            format!(
                                "In spec '{}': measure type '{}' has inconsistent unit decompositions. \
                                 Unit '{}' is a simple unit (decomposition {}) but other units \
                                 have decomposition {}.",
                                spec_name,
                                type_name,
                                unit.name,
                                semantics::format_signature_operator_style(
                                    &semantics::base_measure_vector_as_signature(&simple_decomp)
                                ),
                                semantics::format_signature_operator_style(
                                    &semantics::base_measure_vector_as_signature(existing)
                                )
                            ),
                            type_source.clone(),
                            None::<String>,
                        ));
                    }
                } else {
                    resolved_type_decomp = Some(simple_decomp);
                }

                if let Some(factor) = unit.literal_factor() {
                    resolved_unit_scales[unit_idx] =
                        Some(ResolvedCompoundScale::Literal(factor.clone()));
                }
                continue;
            }

            if body_already_resolved && !unit_owned_locally(&unit.name) {
                if let Some(factor) = unit.literal_factor() {
                    resolved_unit_scales[unit_idx] =
                        Some(ResolvedCompoundScale::Literal(factor.clone()));
                } else if unit.is_dynamic() {
                    resolved_unit_scales[unit_idx] = Some(ResolvedCompoundScale::Dynamic {
                        scale: unit.scale.clone(),
                        coefficient: unit.scale_coefficient(),
                        exponent: unit.scale_exponent(),
                    });
                }
                continue;
            }

            match resolve_compound_unit(
                spec_name,
                type_name,
                unit,
                &lookup,
                unit_index,
                &resolved,
                type_source.as_ref(),
            ) {
                Ok((unit_decomp, derived_scale)) => {
                    if let [(referenced, 1)] = unit.derived_measure_factors.as_slice() {
                        let (bare_name, owner) = unit_index
                            .resolve_owner(referenced, &resolved)
                            .unwrap_or_else(|err| {
                                panic!(
                                    "BUG: factor '{referenced}' resolved for its conversion but not for its owner: {err}"
                                )
                            });
                        let owning_type = owner.lemma_type(&resolved);
                        let owner_family = owning_type
                            .measure_family()
                            .expect("BUG: resolved unit owner is not a named measure");
                        let declaring_family = resolved[type_name]
                            .measure_family()
                            .expect("BUG: declaring measure has no family");
                        if owner_family != declaring_family {
                            unit_errors.push(Error::validation(
                                format!(
                                    "In spec '{spec_name}': unit '{}' in measure type '{type_name}' cannot use a single unit from another measure. '{bare_name}' belongs to '{}'.",
                                    unit.name, owner_family.name
                                ),
                                type_source.clone(),
                                None::<String>,
                            ));
                        }
                    }
                    // Same-family scale (`workday: 8 hour` on duration): resolved decomp matches
                    // the declaring type's declared decomp → simple identity + expanded factor.
                    if unit_decomp == declared_measure_decomposition(resolved[type_name].as_ref()) {
                        resolve_to_simple[unit_idx] = true;
                    }
                    if let Some(existing) = &resolved_type_decomp {
                        if existing != &unit_decomp {
                            unit_errors.push(Error::validation(
                                format!(
                                    "In spec '{}': measure type '{}' has inconsistent unit \
                                     decompositions. Unit '{}' resolved to {:?} but other units \
                                     resolved to {:?}. All units of a measure must measure the same \
                                     physical measure.",
                                    spec_name, type_name, unit.name, unit_decomp, existing
                                ),
                                type_source.clone(),
                                None::<String>,
                            ));
                        }
                    } else {
                        resolved_type_decomp = Some(unit_decomp);
                    }

                    resolved_unit_scales[unit_idx] = Some(derived_scale);
                }
                Err(err) => unit_errors.push(err),
            }
        }

        if !unit_errors.is_empty() {
            errors.extend(unit_errors);
            continue;
        }

        let type_decomp = match resolved_type_decomp {
            Some(d) => d,
            None => continue,
        };

        let owned = unwrap_unique_resolved_type(
            resolved
                .shift_remove(type_name)
                .expect("BUG: type_name comes from resolved's own keys"),
        );

        let updated = owned.map_measure(|units, _| {
            let units = MeasureUnits(
                units
                    .0
                    .into_iter()
                    .enumerate()
                    .map(|(idx, u)| {
                        let u = u.with_decomposition(type_decomp.clone());
                        let u = match resolved_unit_scales[idx].clone() {
                            Some(ResolvedCompoundScale::Literal(factor)) => u.with_factor(factor),
                            Some(ResolvedCompoundScale::Dynamic {
                                scale,
                                coefficient,
                                exponent,
                            }) => u.with_dynamic_scale(scale, coefficient, exponent),
                            None => u,
                        };
                        if resolve_to_simple[idx] {
                            u.with_derived_measure_factors(Vec::new())
                        } else {
                            u
                        }
                    })
                    .collect(),
            );
            (units, Some(type_decomp.clone()))
        });

        if let TypeSpecification::Measure { units, .. } = &updated.specifications {
            for unit in units.iter() {
                // Import clones: only additive (locally owned) units may claim bare names in
                // lookup. Inherited compounds stay keyed to the defining import owner.
                if body_already_resolved && !unit_owned_locally(&unit.name) {
                    continue;
                }
                let Some(factor) = unit.literal_factor().cloned() else {
                    continue;
                };
                lookup.insert(
                    unit.name.clone(),
                    (Some(type_name.clone()), type_decomp.clone(), factor),
                );
            }
        }
        resolved.insert(type_name.clone(), Arc::new(updated));
    }

    let resolved = canonicalize_unit_signatures(resolved);

    (resolved, errors)
}

/// Functional wrapper around the still-`&mut`-based
/// [`semantics::finalize_measure_unit_constraint_magnitudes`]. Consumes the
/// `LemmaType`, performs the validation, and returns either the updated value
/// or an error message (the type is dropped on failure).
fn finalize_lemma_measure_magnitudes(
    lemma_type: LemmaType,
    declared_suggestion: Option<&BoundValueKind>,
    type_name: &str,
) -> Result<LemmaType, String> {
    let LemmaType {
        name,
        mut specifications,
        extends,
        options,
    } = lemma_type;
    semantics::finalize_measure_unit_constraint_magnitudes(
        &mut specifications,
        declared_suggestion.map(|bound| &bound.value),
        type_name,
    )?;
    Ok(LemmaType {
        name,
        specifications,
        extends,
        options,
    })
}

fn finalize_measure_magnitudes_in_resolved(
    resolved: IndexMap<String, Arc<LemmaType>>,
    declared_suggestions: &IndexMap<String, BoundValueKind>,
    type_sources: &HashMap<String, Source>,
    spec_name: &str,
    spec: &LemmaSpec,
) -> (IndexMap<String, Arc<LemmaType>>, Vec<Error>) {
    resolved.into_iter().fold(
        (IndexMap::new(), Vec::new()),
        |(mut acc, mut errors), (type_name, arc)| {
            let source = type_sources.get(&type_name).cloned().unwrap_or_else(|| {
                unreachable!(
                    "BUG: resolved type '{}' has no corresponding DataTypeDef in spec '{}'",
                    type_name, spec_name
                )
            });
            let owned = unwrap_unique_resolved_type(arc);
            let lemma_type = match finalize_lemma_measure_magnitudes(
                owned.clone(),
                declared_suggestions.get(&type_name),
                &type_name,
            ) {
                Ok(lt) => lt,
                Err(message) => {
                    errors.push(Error::validation_with_context(
                        format!(
                            "Type '{}' has invalid measure unit constraints: {}",
                            type_name, message
                        ),
                        Some(source),
                        None::<String>,
                        Some(spec),
                        None,
                    ));
                    owned
                }
            };
            acc.insert(type_name, Arc::new(lemma_type));
            (acc, errors)
        },
    )
}

/// Populate every unit's `derived_measure_factors` to its canonical symbolic signature.
///
/// - Compound declarations keep their parsed factors, canonicalized.
/// - Simple units (empty `derived_measure_factors`) get `[(unit.name, 1)]`.
///
/// After this pass every unit carries a non-empty, canonical-form signature so cross-type
/// arithmetic can combine them deterministically and `signature_index` can be built.
fn canonicalize_unit_signatures(types: TypeMap) -> TypeMap {
    types
        .into_iter()
        .map(|(name, arc)| {
            (
                name,
                Arc::new(canonicalize_lemma_unit_signatures(
                    unwrap_unique_resolved_type(arc),
                )),
            )
        })
        .collect()
}

fn canonicalize_lemma_unit_signatures(lemma_type: LemmaType) -> LemmaType {
    lemma_type.map_measure(|units, decomposition| {
        let units = units.map(canonicalize_unit_for_measure);
        (units, decomposition)
    })
}

fn reserved_calendar_units_in_measure(lemma_type: &LemmaType) -> Vec<String> {
    let TypeSpecification::Measure { units, .. } = &lemma_type.specifications else {
        return Vec::new();
    };
    units
        .iter()
        .filter(|u| calendar_unit_factor(&u.name).is_some())
        .map(|u| u.name.clone())
        .collect()
}

fn measure_unit_is_simple(unit: &MeasureUnit) -> bool {
    unit.derived_measure_factors.is_empty()
        || (unit.derived_measure_factors.len() == 1
            && unit.derived_measure_factors[0].0 == unit.name
            && unit.derived_measure_factors[0].1 == 1)
}

fn canonicalize_unit_for_measure(unit: MeasureUnit) -> MeasureUnit {
    let factors = if measure_unit_is_simple(&unit) {
        vec![(unit.name.clone(), 1)]
    } else {
        canonicalize_signature(&unit.derived_measure_factors)
    };
    unit.with_derived_measure_factors(factors)
}

/// Result of resolving a compound unit's numeric scale.
#[derive(Clone, Debug)]
enum ResolvedCompoundScale {
    Literal(RationalInteger),
    /// Inherits a dynamic base unit's reference; effective factor is
    /// `settled(reference)^exponent * coefficient`.
    Dynamic {
        scale: crate::literals::UnitScale,
        coefficient: RationalInteger,
        exponent: i32,
    },
}

fn resolve_compound_unit(
    spec_name: &str,
    declaring_type_name: &str,
    unit: &MeasureUnit,
    lookup: &UnitDecompLookup,
    unit_index: &PlanningUnitIndex,
    resolved: &IndexMap<String, Arc<LemmaType>>,
    source: Option<&Source>,
) -> Result<(BaseMeasureVector, ResolvedCompoundScale), Error> {
    use crate::computation::bigint::BigInt;
    use crate::computation::rational::{checked_mul, checked_pow_integer, rational_one};

    let mut result: BaseMeasureVector = BaseMeasureVector::new();
    let mut coefficient = unit.literal_factor().cloned().unwrap_or_else(rational_one);
    let mut dynamic: Option<(crate::literals::UnitScale, i32)> = None;

    for (measure_ref, exponent) in &unit.derived_measure_factors {
        let (owning_measure_name, owning_decomp, component) = if let Some(entry) =
            lookup.get(measure_ref.as_str())
        {
            (
                entry.0.clone(),
                entry.1.clone(),
                CompoundComponent::Literal(entry.2.clone()),
            )
        } else {
            let (bare, owner) = unit_index
                .resolve_owner(measure_ref, resolved)
                .map_err(|err| {
                    Error::validation(
                        format!(
                            "In spec '{}': unit '{}' in measure type '{}' references '{}': {}. \
                             Add `uses <spec>` (or declare the owning measure type in this spec) \
                             so its units are in scope.",
                            spec_name, unit.name, declaring_type_name, measure_ref, err
                        ),
                        source.cloned(),
                        None::<String>,
                    )
                })?;
            let owning_type = owner.lemma_type(resolved);
            let TypeSpecification::Measure {
                decomposition: Some(decomposition),
                units,
                ..
            } = &owning_type.specifications
            else {
                return Err(Error::validation(
                    format!(
                        "In spec '{}': unit '{}' in measure type '{}' references '{}' which did not resolve to a measure unit",
                        spec_name, unit.name, declaring_type_name, measure_ref
                    ),
                    source.cloned(),
                    None::<String>,
                ));
            };
            let referenced = units.iter().find(|u| u.name == bare).unwrap_or_else(|| {
                panic!(
                    "BUG: resolved unit '{}' must be declared on type '{}'",
                    bare,
                    owning_type.name()
                )
            });
            let component = if let Some(factor) = referenced.literal_factor().cloned() {
                CompoundComponent::Literal(factor)
            } else {
                CompoundComponent::Dynamic {
                    scale: referenced.scale.clone(),
                    bare: bare.to_string(),
                    coefficient: referenced.scale_coefficient(),
                    exponent: referenced.scale_exponent(),
                }
            };
            (local_owner_name(owner), decomposition.clone(), component)
        };
        let owning_decomp = &owning_decomp;

        if owning_measure_name.as_deref() == Some(declaring_type_name) {
            return Err(Error::validation(
                format!(
                    "In spec '{}': unit '{}' in measure type '{}' references unit '{}' which \
                     belongs to the same measure type. A measure cannot reference its own units \
                     in a compound expression.",
                    spec_name, unit.name, declaring_type_name, measure_ref
                ),
                source.cloned(),
                None::<String>,
            ));
        }

        for (dim, &dim_exp) in owning_decomp {
            let combined = dim_exp
                .checked_mul(*exponent)
                .and_then(|value| accumulate(&mut result, dim, value));
            if combined.is_none() {
                return Err(unit_exponent_limit_error(
                    spec_name,
                    &unit.name,
                    declaring_type_name,
                    source,
                ));
            }
        }

        match component {
            CompoundComponent::Literal(unit_factor) => {
                let component_contribution =
                    checked_pow_integer(&unit_factor, &BigInt::from_i64(i64::from(*exponent)))
                        .map_err(|error| {
                            overflow_to_validation_error(
                                spec_name,
                                &unit.name,
                                declaring_type_name,
                                measure_ref,
                                error,
                                source,
                            )
                        })?;
                coefficient =
                    checked_mul(&coefficient, &component_contribution).map_err(|error| {
                        overflow_to_validation_error(
                            spec_name,
                            &unit.name,
                            declaring_type_name,
                            measure_ref,
                            error,
                            source,
                        )
                    })?;
            }
            CompoundComponent::Dynamic {
                scale,
                bare,
                coefficient: base_coefficient,
                exponent: base_exponent,
            } => {
                let raised_coefficient =
                    checked_pow_integer(&base_coefficient, &BigInt::from_i64(i64::from(*exponent)))
                        .map_err(|error| {
                            overflow_to_validation_error(
                                spec_name,
                                &unit.name,
                                declaring_type_name,
                                measure_ref,
                                error,
                                source,
                            )
                        })?;
                coefficient = checked_mul(&coefficient, &raised_coefficient).map_err(|error| {
                    overflow_to_validation_error(
                        spec_name,
                        &unit.name,
                        declaring_type_name,
                        measure_ref,
                        error,
                        source,
                    )
                })?;
                let component_exponent = base_exponent
                    .checked_mul(*exponent)
                    .ok_or_else(|| {
                        Error::validation(
                            format!(
                                "In spec '{spec_name}': unit '{}' in measure type '{declaring_type_name}' overflowed combining dynamic unit '{bare}' exponents.",
                                unit.name
                            ),
                            source.cloned(),
                            None::<String>,
                        )
                    })?;
                match &mut dynamic {
                    None => {
                        dynamic = Some((scale, component_exponent));
                    }
                    Some((existing_scale, existing_exponent)) => {
                        if !same_dynamic_scale_identity(existing_scale, &scale) {
                            return Err(Error::validation(
                                format!(
                                    "In spec '{spec_name}': unit '{}' in measure type '{declaring_type_name}' combines distinct dynamic units in one compound expression.",
                                    unit.name
                                ),
                                source.cloned(),
                                None::<String>,
                            ));
                        }
                        *existing_exponent = existing_exponent
                            .checked_add(component_exponent)
                            .ok_or_else(|| {
                                Error::validation(
                                    format!(
                                        "In spec '{spec_name}': unit '{}' in measure type '{declaring_type_name}' overflowed combining dynamic unit '{bare}' exponents.",
                                        unit.name
                                    ),
                                    source.cloned(),
                                    None::<String>,
                                )
                            })?;
                    }
                }
            }
        }
    }

    let derived_scale = match dynamic {
        None => ResolvedCompoundScale::Literal(coefficient),
        Some((scale, exponent)) => ResolvedCompoundScale::Dynamic {
            scale,
            coefficient,
            exponent,
        },
    };
    Ok((result, derived_scale))
}

enum CompoundComponent {
    Literal(RationalInteger),
    Dynamic {
        scale: crate::literals::UnitScale,
        bare: String,
        coefficient: RationalInteger,
        exponent: i32,
    },
}

fn same_dynamic_scale_identity(
    left: &crate::literals::UnitScale,
    right: &crate::literals::UnitScale,
) -> bool {
    match (left, right) {
        (
            crate::literals::UnitScale::Unresolved {
                segments: left_segments,
                name: left_name,
                ..
            },
            crate::literals::UnitScale::Unresolved {
                segments: right_segments,
                name: right_name,
                ..
            },
        ) => left_segments == right_segments && left_name == right_name,
        (
            crate::literals::UnitScale::Reference {
                target: left_target,
                ..
            },
            crate::literals::UnitScale::Reference {
                target: right_target,
                ..
            },
        ) => left_target == right_target,
        _ => false,
    }
}

fn overflow_to_validation_error(
    spec_name: &str,
    unit_name: &str,
    declaring_type_name: &str,
    measure_ref: &str,
    failure: NumericFailure,
    source: Option<&Source>,
) -> Error {
    Error::validation(
        format!(
            "In spec '{}': unit '{}' in measure type '{}' overflowed while combining '{}': {}",
            spec_name, unit_name, declaring_type_name, measure_ref, failure
        ),
        source.cloned(),
        None::<String>,
    )
}

fn unit_exponent_limit_error(
    spec_name: &str,
    unit_name: &str,
    declaring_type_name: &str,
    source: Option<&Source>,
) -> Error {
    Error::validation(
        format!(
            "In spec '{}': unit '{}' in measure type '{}' has a unit exponent exceeding the maximum of {}",
            spec_name,
            unit_name,
            declaring_type_name,
            MAX_UNIT_EXPONENT
        ),
        source.cloned(),
        None::<String>,
    )
}

/// `None` when the accumulated exponent leaves [`crate::limits::MAX_UNIT_EXPONENT`].
fn accumulate(
    result: &mut BaseMeasureVector,
    dim: &semantics::Dimension,
    value: i32,
) -> Option<()> {
    let entry = result.entry(dim.clone()).or_insert(0);
    *entry = entry.checked_add(value)?;
    if entry.unsigned_abs() > MAX_UNIT_EXPONENT.unsigned_abs() {
        return None;
    }
    if *entry == 0 {
        result.remove(dim);
    }
    Some(())
}

// Type resolution
// ============================================================================

/// Fully resolved types for a single spec.
/// After resolution, all imports are inlined — specs are independent.
#[derive(Debug, Clone, Default)]
pub struct ResolvedSpecTypes {
    /// Resolved [`LemmaType`] for each **data type row name** declared in this spec (`data name: …`).
    /// Planning-only: includes measure units and post-`resolve_measure_decompositions` decomposition.
    pub resolved: IndexMap<String, Arc<LemmaType>>,

    /// Declared default per named type (e.g. `type rate: ratio -> suggest 50%`).
    /// Only present for types that declared a `-> suggest ...` constraint anywhere
    /// in their extension chain; the inner-most `-> suggest` wins. Defaults live
    /// outside [`TypeSpecification`] so the type itself stays free of binding data.
    /// Populated after [`bound_value_kind_from_raw_suggestion`] (post-decomposition).
    pub declared_suggestions: IndexMap<String, BoundValueKind>,

    /// Declared value per named type (e.g. `type rate: ratio -> with 50%`).
    /// Only present for types that declared a `-> with ...` constraint anywhere
    /// in their extension chain; the inner-most `-> with` wins.
    pub declared_values: IndexMap<String, BoundValueKind>,

    /// Defaults captured during type resolution, before measure unit factors are final.
    pub(crate) raw_suggestions: Vec<(String, RawSuggestion)>,

    /// Raw fills captured during type resolution, before measure unit factors are final.
    pub(crate) raw_values: Vec<(String, RawSuggestion)>,

    /// Raw defaults retained after [`bound_value_kind_from_raw_suggestion`] for cross-spec parent lookup during later specs.
    pub(crate) source_defaults: IndexMap<String, RawSuggestion>,

    /// Raw fills retained for cross-spec parent lookup during later specs.
    pub(crate) source_value_defaults: IndexMap<String, RawSuggestion>,

    /// List types written in this spec's rules (`money list -> with ...`), keyed by the
    /// written type, in order of first appearance.
    pub(crate) rule_list_types: Vec<(ParentType, Arc<LemmaType>)>,

    /// Expression-scope units. One declarer per bare unit name; qualify with Type.unit or alias.Type.unit.
    /// Binding aliases never appear as index keys.
    pub unit_index: crate::planning::unit_index::UnitIndex,

    /// Cross-type unit-signature collisions. Emitted after rules are typed, unless a
    /// rule result is already the anonymous measure of that decomposition.
    pub(crate) deferred_signature_collisions:
        Vec<(String, crate::planning::semantics::BaseMeasureVector)>,

    /// Names of unit owners until [`PlanningUnitIndex::bind`] after magnitudes.
    planning_unit_index: PlanningUnitIndex,

    /// Reverse index: canonical-form unit signature → (unit_name, owning type).
    /// Built from [`Self::unit_index`] during resolve; cloned into the execution plan.
    pub signature_index: crate::computation::arithmetic::SignatureIndex,
}

/// Intermediate type definition extracted from [`DataValue::Definition`] data.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct DataTypeDef {
    pub parent: ParentType,
    pub constraints: Option<Vec<Constraint>>,
    pub source: crate::parsing::source::Source,
    pub name: String,
    /// When the source row was `data N: <literal>` (no explicit parent type), the AST literal.
    pub bound_literal: Option<ast::Value>,
}

///
/// Named types are extracted from [`DataValue::Definition`] data and stored in
/// DAG-parallel maps keyed by Context-owned `&LemmaSpec` identity.
#[derive(Debug, Clone)]
pub(crate) struct TypeResolver<'a> {
    data_types: Vec<(&'a LemmaSpec, HashMap<String, DataTypeDef>)>,
    context: &'a Context,
    all_registered_specs: Vec<(Arc<LemmaRepository>, &'a LemmaSpec)>,
    /// Specs for which [`TypeResolver::register_all`] could not register every
    /// type-declaring data row. Planning must skip building these specs:
    /// downstream code (`add_data`) relies on every type-resolving data row
    /// being present in the resolved type map.
    specs_with_failed_registration: Vec<&'a LemmaSpec>,
    /// Slices of every spec version in the dependency order of the graph being built.
    slices: HashMap<SpecVersionKey, Arc<instance_list::VersionSlices>>,
}

/// Parent type used for measure-family lookup, unwrapping [`ParentType::Ranged`].
fn element_parent_type(parent: &ParentType) -> &ParentType {
    let mut current = parent;
    while let ParentType::Ranged { inner } | ParentType::List { inner } = current {
        current = inner.as_ref();
    }
    current
}

pub(crate) fn time_range_endpoints_share_timezone(
    left: &semantics::SemanticTime,
    right: &semantics::SemanticTime,
) -> bool {
    match (&left.timezone, &right.timezone) {
        (None, None) => true,
        (Some(left_tz), Some(right_tz)) => left_tz.offset_minutes == right_tz.offset_minutes,
        _ => false,
    }
}

/// Display name for a parser literal's base type, matching the names produced
/// by [`LemmaType::name`] for the corresponding semantic literal. Used in
/// range endpoint validation error messages.
fn parser_value_base_type_display_name(value: &ast::Value) -> &'static str {
    match value {
        ast::Value::Number(_) => "number",
        ast::Value::Text(_) => "text",
        ast::Value::Boolean(_) => "boolean",
        ast::Value::Date(_) => "date",
        ast::Value::Time(_) => "time",
        ast::Value::NumberWithUnit(_, unit) => match unit.as_str() {
            "percent" | "permille" => "ratio",
            _ => "measure",
        },
        ast::Value::Range(_, _) => "range",
    }
}

/// Infer primitive [`ParentType`] from a literal RHS (`data x: 3.14`).
///
/// Returns an error for range literals whose endpoint types are not a
/// supported range element combination (e.g. `1 ... yes`, `"a" ... "b"`).
fn inferred_parent_type_from_literal(value: &ast::Value) -> Result<ParentType, String> {
    let parent_type = match value {
        ast::Value::Number(_) => ParentType::Primitive {
            primitive: PrimitiveKind::Number,
        },
        ast::Value::Text(_) => ParentType::Primitive {
            primitive: PrimitiveKind::Text,
        },
        ast::Value::Boolean(_) => ParentType::Primitive {
            primitive: PrimitiveKind::Boolean,
        },
        ast::Value::Date(_) => ParentType::Primitive {
            primitive: PrimitiveKind::Date,
        },
        ast::Value::Time(_) => ParentType::Primitive {
            primitive: PrimitiveKind::Time,
        },
        ast::Value::NumberWithUnit(_, _) => ParentType::Primitive {
            primitive: PrimitiveKind::Measure,
        },
        ast::Value::Range(left, right) => {
            let primitive = match (left.as_ref(), right.as_ref()) {
                (ast::Value::Number(_), ast::Value::Number(_)) => PrimitiveKind::NumberRange,
                (ast::Value::Date(_), ast::Value::Date(_)) => PrimitiveKind::DateRange,
                (ast::Value::Time(_), ast::Value::Time(_)) => PrimitiveKind::TimeRange,
                (ast::Value::NumberWithUnit(_, u1), ast::Value::NumberWithUnit(_, u2))
                    if u1 == u2 && matches!(u1.as_str(), "percent" | "permille") =>
                {
                    PrimitiveKind::RatioRange
                }
                (ast::Value::NumberWithUnit(_, _), ast::Value::NumberWithUnit(_, _)) => {
                    PrimitiveKind::MeasureRange
                }
                (left_value, right_value) => {
                    return Err(format!(
                        "range endpoints must have the same supported base type, got {} and {}",
                        parser_value_base_type_display_name(left_value),
                        parser_value_base_type_display_name(right_value)
                    ));
                }
            };
            ParentType::Primitive { primitive }
        }
    };
    Ok(parent_type)
}

impl<'a> TypeResolver<'a> {
    pub fn new(
        context: &'a Context,
        slices: HashMap<SpecVersionKey, Arc<instance_list::VersionSlices>>,
    ) -> Self {
        TypeResolver {
            data_types: Vec::new(),
            context,
            all_registered_specs: Vec::new(),
            specs_with_failed_registration: Vec::new(),
            slices,
        }
    }

    /// Identity of `spec`, resolved at `at`, in the slice that holds `at`.
    fn identity_at(
        &self,
        repository: &LemmaRepository,
        spec: &LemmaSpec,
        at: &EffectiveDate,
    ) -> SpecIdentity {
        self.slices
            .get(&SpecVersionKey::of(repository, spec))
            .expect("BUG: a spec type resolution reaches is in the graph's dependency order")
            .identity_at(repository, spec, at)
    }

    /// Spec-element `name list` written in a rule: list type with Spec element (numbered key;
    /// `-> key` refines the key during conversion).
    fn rule_spec_element_list_type(
        &self,
        spec: &LemmaSpec,
        parent: &ParentType,
        source: &Source,
        at: &EffectiveDate,
    ) -> Option<Arc<LemmaType>> {
        let (name, spec_ref) = Self::list_of_spec_name(spec, parent)?;
        let (row_repository, row_spec) = match discovery::resolve_spec_ref_or_self(
            self.context,
            &spec_ref,
            self.registered_repository(spec),
            spec,
            at,
            Some(source.clone()),
        ) {
            Ok(pair) => pair,
            Err(_) => return None,
        };
        let identity = self.identity_at(&row_repository, row_spec, &spec_ref.at(at));
        let element = Arc::new(LemmaType::primitive(TypeSpecification::Spec {
            spec: identity,
        }));
        Some(Arc::new(LemmaType {
            name: Some(format!("{name} list")),
            specifications: TypeSpecification::List {
                element,
                key: ListKey::Numbered,
                help: String::new(),
            },
            extends: TypeExtends::Primitive,
            options: TypeOptions::default(),
        }))
    }

    /// `name` and its row spec when `parent` is `name list` of spec rows: `name` is a local
    /// name of a `uses` line (`uses tb: tax_bracket` then `data brackets: tb list`) or the
    /// spec's own name ([`discovery::list_element_spec_ref`]).
    fn list_of_spec_name<'p>(
        spec: &LemmaSpec,
        parent: &'p ParentType,
    ) -> Option<(&'p str, ast::SpecRef)> {
        let ParentType::List { inner } = parent else {
            return None;
        };
        let ParentType::Custom { name } = inner.as_ref() else {
            return None;
        };
        discovery::list_element_spec_ref(spec, inner).map(|spec_ref| (name.as_str(), spec_ref))
    }

    pub fn is_registered(&self, spec: &LemmaSpec) -> bool {
        self.all_registered_specs
            .iter()
            .any(|(_, s)| discovery::same_loaded_spec(s, spec))
    }

    /// Repository `spec` was registered under by [`Self::register_all`].
    fn registered_repository(&self, spec: &LemmaSpec) -> &Arc<LemmaRepository> {
        self.all_registered_specs
            .iter()
            .find(|(_, registered)| discovery::same_loaded_spec(registered, spec))
            .map(|(repository, _)| repository)
            .expect("BUG: types are resolved only for specs registered with their repository")
    }

    /// Record that a type-declaring data row of `spec` could not be registered.
    fn mark_registration_failure(&mut self, spec: &'a LemmaSpec) {
        if !self
            .specs_with_failed_registration
            .iter()
            .any(|s| discovery::same_loaded_spec(s, spec))
        {
            self.specs_with_failed_registration.push(spec);
        }
    }

    /// Whether [`TypeResolver::register_all`] failed to register a
    /// type-declaring data row of `spec`. The corresponding errors were
    /// returned by `register_all`; specs with failed registrations must not
    /// be built.
    pub fn registration_failed(&self, spec: &LemmaSpec) -> bool {
        self.specs_with_failed_registration
            .iter()
            .any(|s| discovery::same_loaded_spec(s, spec))
    }

    /// Register all type-declaring data from a spec.
    pub fn register_all(
        &mut self,
        repository: &Arc<LemmaRepository>,
        spec: &'a LemmaSpec,
    ) -> Vec<Error> {
        self.record_repository(repository, spec);

        let mut errors = Vec::new();
        for data in &spec.data {
            match &data.value {
                ParsedDataValue::Definition {
                    base,
                    constraints,
                    value,
                    list: _,
                } => {
                    if matches!(
                        (base.as_ref(), constraints.as_ref(), value.as_ref()),
                        (None, None, Some(Value::NumberWithUnit(_, _)),)
                    ) {
                        continue;
                    }
                    let name = &data.reference.name;
                    let parent = match (base.as_ref(), value.as_ref()) {
                        (Some(b), _) => b.clone(),
                        (None, Some(v)) => match inferred_parent_type_from_literal(v) {
                            Ok(parent) => parent,
                            Err(message) => {
                                errors.push(Error::validation_with_context(
                                    message,
                                    Some(data.source_location.clone()),
                                    None::<String>,
                                    Some(spec),
                                    None,
                                ));
                                self.mark_registration_failure(spec);
                                continue;
                            }
                        },
                        (None, None) => {
                            errors.push(Error::validation_with_context(
                                format!(
                                    "Data '{name}' in spec '{}' must declare a type or a literal value",
                                    spec.name
                                ),
                                Some(data.source_location.clone()),
                                None::<String>,
                                Some(spec),
                                None,
                            ));
                            self.mark_registration_failure(spec);
                            continue;
                        }
                    };
                    // Spec-element instance lists become DataDefinition::InstanceList in
                    // add_instance_list; they are not named types in the type registry.
                    if Self::list_of_spec_name(spec, &parent).is_some() {
                        continue;
                    }
                    let ftd = DataTypeDef {
                        parent,
                        constraints: constraints.clone(),
                        source: data.source_location.clone(),
                        name: name.clone(),
                        bound_literal: value.clone(),
                    };
                    if let Err(e) = self.register_type(repository, spec, ftd) {
                        errors.push(e);
                    }
                }
                ParsedDataValue::Import { .. } | ParsedDataValue::ImportList { .. } => {}
            }
        }
        errors
    }

    fn record_repository(&mut self, repository: &Arc<LemmaRepository>, spec: &'a LemmaSpec) {
        if !self
            .all_registered_specs
            .iter()
            .any(|(_, s)| discovery::same_loaded_spec(s, spec))
        {
            self.all_registered_specs
                .push((Arc::clone(repository), spec));
        }
    }

    /// Register a type from a data declaration of `spec`, loaded under `repository`.
    pub fn register_type(
        &mut self,
        repository: &Arc<LemmaRepository>,
        spec: &'a LemmaSpec,
        def: DataTypeDef,
    ) -> Result<(), Error> {
        self.record_repository(repository, spec);
        let spec_types = if let Some(pos) = self
            .data_types
            .iter()
            .position(|(s, _)| discovery::same_loaded_spec(s, spec))
        {
            &mut self.data_types[pos].1
        } else {
            self.data_types.push((spec, HashMap::new()));
            let last = self.data_types.len() - 1;
            &mut self.data_types[last].1
        };
        if spec_types.contains_key(&def.name) {
            return Err(Error::validation_with_context(
                format!(
                    "The name '{}' is already used for data in this spec.",
                    def.name
                ),
                Some(def.source.clone()),
                None::<String>,
                Some(spec),
                None,
            ));
        }
        spec_types.insert(def.name.clone(), def);
        Ok(())
    }

    /// Resolve types for a single spec and validate their specifications.
    /// `at` is the planning instant for this spec (nested qualified refs use their pin).
    pub fn resolve_and_validate(
        &self,
        spec: &LemmaSpec,
        at: &EffectiveDate,
        already_resolved: &ResolvedTypesMap,
    ) -> Result<ResolvedSpecTypes, Vec<Error>> {
        let (mut resolved_types, list_type_names) =
            self.resolve_types_internal(spec, at, already_resolved)?;
        resolved_types.source_defaults = resolved_types
            .raw_suggestions
            .iter()
            .map(|(name, raw)| (name.clone(), raw.clone()))
            .collect();
        resolved_types.source_value_defaults = resolved_types
            .raw_values
            .iter()
            .map(|(name, raw)| (name.clone(), raw.clone()))
            .collect();
        let mut errors = Vec::new();

        // Build the type-name → source map for precise error reporting.
        let type_sources: std::collections::HashMap<String, Source> = resolved_types
            .resolved
            .keys()
            .chain(list_type_names.iter())
            .filter_map(|type_name| {
                self.data_types
                    .iter()
                    .find(|(s, _)| discovery::same_loaded_spec(s, spec))
                    .and_then(|(_, defs)| defs.get(type_name.as_str()))
                    .map(|ftd| (type_name.clone(), ftd.source.clone()))
            })
            .collect();

        // Run the decomposition pass to populate `BaseMeasureVector` on all Measure types.
        errors.extend(rewrite_unit_path_scale_payloads(
            self,
            spec,
            at,
            already_resolved,
            &mut resolved_types.resolved,
            &resolved_types.planning_unit_index,
            &type_sources,
        ));
        let this_spec = SpecVersionKey::of(self.registered_repository(spec), spec);
        let (new_resolved, decomp_errors) = resolve_measure_decompositions(
            &spec.name,
            &this_spec,
            std::mem::take(&mut resolved_types.resolved),
            &resolved_types.planning_unit_index,
            &type_sources,
        );
        resolved_types.resolved = new_resolved;
        errors.extend(decomp_errors);

        for (type_name, raw) in std::mem::take(&mut resolved_types.raw_suggestions) {
            let lemma_type = resolved_types
                .resolved
                .get(&type_name)
                .expect("BUG: raw default for type not in resolved");
            match bound_value_kind_from_raw_suggestion(raw, &lemma_type.specifications, &type_name)
            {
                Ok(value_kind) => {
                    resolved_types
                        .declared_suggestions
                        .insert(type_name, value_kind);
                }
                Err(message) => {
                    let source = type_sources
                        .get(&type_name)
                        .cloned()
                        .unwrap_or_else(|| unreachable!("BUG: type '{}' has no source", type_name));
                    errors.push(Error::validation_with_context(
                        message,
                        Some(source),
                        None::<String>,
                        Some(spec),
                        None,
                    ));
                }
            }
        }

        for (type_name, raw) in std::mem::take(&mut resolved_types.raw_values) {
            let lemma_type = resolved_types
                .resolved
                .get(&type_name)
                .expect("BUG: raw fill for type not in resolved");
            match bound_value_kind_from_raw_suggestion(raw, &lemma_type.specifications, &type_name)
            {
                Ok(value_kind) => {
                    resolved_types.declared_values.insert(type_name, value_kind);
                }
                Err(message) => {
                    let source = type_sources
                        .get(&type_name)
                        .cloned()
                        .unwrap_or_else(|| unreachable!("BUG: type '{}' has no source", type_name));
                    errors.push(Error::validation_with_context(
                        message,
                        Some(source),
                        None::<String>,
                        Some(spec),
                        None,
                    ));
                }
            }
        }

        if let Some((_, data_defs)) = self
            .data_types
            .iter()
            .find(|(s, _)| discovery::same_loaded_spec(s, spec))
        {
            errors.extend(refresh_named_range_specs(
                self,
                spec,
                data_defs,
                &mut resolved_types,
                already_resolved,
                at,
            ));
            errors.extend(apply_deferred_named_range_constraints(
                spec,
                data_defs,
                &mut resolved_types.resolved,
                &mut resolved_types.declared_suggestions,
                &mut resolved_types.declared_values,
                &type_sources,
                &resolved_types.planning_unit_index,
            ));
        }

        let (new_resolved, resolved_errors) = finalize_measure_magnitudes_in_resolved(
            std::mem::take(&mut resolved_types.resolved),
            &resolved_types.declared_suggestions,
            &type_sources,
            &spec.name,
            spec,
        );
        resolved_types.resolved = new_resolved;

        resolved_types.unit_index = std::mem::take(&mut resolved_types.planning_unit_index)
            .bind(&resolved_types.resolved, &this_spec);

        match build_signature_index(&spec.name, &resolved_types.unit_index, already_resolved) {
            Ok(built) => {
                resolved_types.signature_index = built.index;
                resolved_types.deferred_signature_collisions = built.deferred_cross_type;
            }
            Err(error) => errors.push(error),
        }

        if !resolved_errors.is_empty() {
            errors.extend(resolved_errors);
            return Err(errors);
        }

        if let Err(list_errors) = self.resolve_list_types(
            spec,
            at,
            already_resolved,
            &list_type_names,
            &mut resolved_types,
        ) {
            errors.extend(list_errors);
            return Err(errors);
        }

        if let Err(rule_list_errors) =
            self.resolve_rule_list_types(spec, at, already_resolved, &mut resolved_types)
        {
            errors.extend(rule_list_errors);
            return Err(errors);
        }

        let mut validated_in_unit_index: HashSet<String> = HashSet::new();
        for lemma_type in resolved_types.unit_index.values() {
            let Some(measure_family) = lemma_type.measure_family_name() else {
                continue;
            };
            if !lemma_type.is_measure()
                || !validated_in_unit_index.insert(measure_family.to_string())
            {
                continue;
            }
            let type_name = lemma_type
                .name
                .as_deref()
                .filter(|name| *name == measure_family)
                .unwrap_or(measure_family);
            let source = type_sources
                .get(type_name)
                .cloned()
                .or_else(|| {
                    self.data_types
                        .iter()
                        .find_map(|(_, defs)| defs.get(type_name).map(|def| def.source.clone()))
                })
                .unwrap_or_else(|| {
                    unreachable!(
                        "BUG: measure type '{}' in unit_index has no DataTypeDef source",
                        type_name
                    )
                });
            errors.extend(validate_type_specifications(
                &lemma_type.specifications,
                resolved_types.declared_suggestions.get(type_name),
                "suggestion",
                type_name,
                &source,
                Some(spec),
                &resolved_types.unit_index,
            ));
            errors.extend(validate_type_specifications(
                &lemma_type.specifications,
                resolved_types.declared_values.get(type_name),
                "with",
                type_name,
                &source,
                Some(spec),
                &resolved_types.unit_index,
            ));
        }

        for (type_name, lemma_type) in &resolved_types.resolved {
            let source = type_sources.get(type_name).cloned().unwrap_or_else(|| {
                unreachable!(
                    "BUG: resolved type '{}' has no corresponding DataTypeDef in spec '{}'",
                    type_name, spec.name
                )
            });
            let mut spec_errors = validate_type_specifications(
                &lemma_type.specifications,
                resolved_types.declared_suggestions.get(type_name),
                "suggestion",
                type_name,
                &source,
                Some(spec),
                &resolved_types.unit_index,
            );
            errors.append(&mut spec_errors);
            let mut fill_errors = validate_type_specifications(
                &lemma_type.specifications,
                resolved_types.declared_values.get(type_name),
                "with",
                type_name,
                &source,
                Some(spec),
                &resolved_types.unit_index,
            );
            errors.append(&mut fill_errors);
            errors.extend(validate_type_options(
                lemma_type,
                resolved_types.declared_suggestions.get(type_name),
                "suggestion",
                type_name,
                &source,
                Some(spec),
                &resolved_types.unit_index,
            ));
            errors.extend(validate_type_options(
                lemma_type,
                resolved_types.declared_values.get(type_name),
                "with",
                type_name,
                &source,
                Some(spec),
                &resolved_types.unit_index,
            ));
        }

        for (type_name, lemma_type) in &resolved_types.resolved {
            let is_range = matches!(
                &lemma_type.specifications,
                TypeSpecification::NumberRange { .. }
                    | TypeSpecification::DateRange { .. }
                    | TypeSpecification::TimeRange { .. }
                    | TypeSpecification::MeasureRange { .. }
                    | TypeSpecification::RatioRange { .. }
            );
            if !is_range {
                continue;
            }
            let source = type_sources.get(type_name).cloned().unwrap_or_else(|| {
                unreachable!(
                    "BUG: resolved type '{}' has no corresponding DataTypeDef in spec '{}'",
                    type_name, spec.name
                )
            });
            if let Some(default_kind) = resolved_types.declared_suggestions.get(type_name.as_str())
            {
                let lit = TypedLiteral {
                    value: default_kind.value.clone(),
                    lemma_type: Arc::clone(lemma_type),
                };
                if let Err(message) = crate::planning::execution_plan::validate_value_against_type(
                    lemma_type.as_ref(),
                    &lit.to_literal(),
                    &resolved_types.unit_index,
                    None,
                ) {
                    errors.push(Error::validation_with_context(
                        format!("Type '{}' suggestion is invalid: {}", type_name, message),
                        Some(source.clone()),
                        None::<String>,
                        Some(spec),
                        None,
                    ));
                }
            }
            if let Some(default_kind) = resolved_types.declared_values.get(type_name.as_str()) {
                let lit = TypedLiteral {
                    value: default_kind.value.clone(),
                    lemma_type: Arc::clone(lemma_type),
                };
                if let Err(message) = crate::planning::execution_plan::validate_value_against_type(
                    lemma_type.as_ref(),
                    &lit.to_literal(),
                    &resolved_types.unit_index,
                    None,
                ) {
                    errors.push(Error::validation_with_context(
                        format!("Type '{}' with is invalid: {}", type_name, message),
                        Some(source),
                        None::<String>,
                        Some(spec),
                        None,
                    ));
                }
            }
        }

        if errors.is_empty() {
            Ok(resolved_types)
        } else {
            Err(errors)
        }
    }

    // =========================================================================
    // Private resolution methods
    // =========================================================================

    /// Resolve list types once every scalar type is final, and convert their declared rows.
    fn resolve_list_types(
        &self,
        spec: &LemmaSpec,
        at: &EffectiveDate,
        already_resolved: &ResolvedTypesMap,
        list_type_names: &[String],
        resolved_types: &mut ResolvedSpecTypes,
    ) -> Result<(), Vec<Error>> {
        if list_type_names.is_empty() {
            return Ok(());
        }
        let data_defs = self
            .data_types
            .iter()
            .find(|(s, _)| discovery::same_loaded_spec(s, spec))
            .map(|(_, defs)| defs)
            .expect("BUG: list type names come from this spec's type definitions");
        let mut local = local_type_entries(resolved_types);
        let mut errors = Vec::new();
        for type_name in list_type_names {
            let ftd = data_defs
                .get(type_name.as_str())
                .expect("BUG: topo-sorted list type missing from registry");
            let (lemma_type, suggestion, value) =
                self.resolve_type_def(spec, type_name, ftd, at, already_resolved, &local)?;
            let to_error = |message: String| {
                Error::validation_with_context(
                    message,
                    Some(ftd.source.clone()),
                    None::<String>,
                    Some(spec),
                    None,
                )
            };
            if let Some(raw) = &suggestion {
                resolved_types
                    .source_defaults
                    .insert(type_name.clone(), raw.clone());
                match bound_value_kind_from_raw_suggestion(
                    raw.clone(),
                    &lemma_type.specifications,
                    type_name,
                ) {
                    Ok(bound) => {
                        resolved_types
                            .declared_suggestions
                            .insert(type_name.clone(), bound);
                    }
                    Err(message) => errors.push(to_error(message)),
                }
            }
            if let Some(raw) = &value {
                resolved_types
                    .source_value_defaults
                    .insert(type_name.clone(), raw.clone());
                match bound_value_kind_from_raw_suggestion(
                    raw.clone(),
                    &lemma_type.specifications,
                    type_name,
                ) {
                    Ok(bound) => {
                        resolved_types
                            .declared_values
                            .insert(type_name.clone(), bound);
                    }
                    Err(message) => errors.push(to_error(message)),
                }
            }
            resolved_types
                .resolved
                .insert(type_name.clone(), Arc::clone(&lemma_type));
            local.insert(type_name.clone(), (lemma_type, suggestion, value));
        }
        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors)
        }
    }

    /// Resolve the list types written in `spec`'s rules against the same scope as its data
    /// types, once every local type is final.
    fn resolve_rule_list_types<'s>(
        &self,
        spec: &'s LemmaSpec,
        at: &EffectiveDate,
        already_resolved: &ResolvedTypesMap,
        resolved_types: &mut ResolvedSpecTypes,
    ) -> Result<(), Vec<Error>> {
        let mut written: Vec<(&'s ParentType, &'s Source)> = Vec::new();
        for rule in &spec.rules {
            let trees = std::iter::once(&rule.expression).chain(
                rule.unless_clauses
                    .iter()
                    .flat_map(|clause| [&clause.condition, &clause.result]),
            );
            for tree in trees {
                let mut pending = vec![tree.root()];
                while let Some(node) = pending.pop() {
                    if let ast::ExpressionKind::ListType { element } = node.kind() {
                        if written.iter().all(|(seen, _)| *seen != element) {
                            let source = node
                                .source()
                                .expect("BUG: AST expression missing source location");
                            written.push((element, source));
                        }
                    }
                    node.push_children(&mut pending);
                }
            }
        }
        if written.is_empty() {
            return Ok(());
        }
        let local = local_type_entries(resolved_types);
        let mut errors = Vec::new();
        for (element, source) in written {
            let scope = ParentLookup {
                resolver: self,
                spec,
                source,
                at,
                already_resolved,
                resolved: &local,
                site: TypeUseSite::RuleListType,
            };
            match lookup_parent_type(&scope, element) {
                Ok((.., lemma_type)) => {
                    resolved_types
                        .rule_list_types
                        .push((element.clone(), lemma_type));
                }
                Err(lookup_errors) => {
                    if let Some(list_type) =
                        self.rule_spec_element_list_type(spec, element, source, at)
                    {
                        resolved_types
                            .rule_list_types
                            .push((element.clone(), list_type));
                    } else {
                        errors.extend(lookup_errors);
                    }
                }
            }
        }
        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors)
        }
    }

    /// Resolve one data type declaration against `resolved` (already resolved local types).
    fn resolve_type_def(
        &self,
        spec: &LemmaSpec,
        type_name: &str,
        ftd: &DataTypeDef,
        at: &EffectiveDate,
        already_resolved: &ResolvedTypesMap,
        resolved: &HashMap<String, ResolvedTypeEntry>,
    ) -> Result<ResolvedTypeEntry, Vec<Error>> {
        let parent = &ftd.parent;
        let constraints = &ftd.constraints;

        let scope = ParentLookup {
            resolver: self,
            spec,
            source: &ftd.source,
            at,
            already_resolved,
            resolved,
            site: TypeUseSite::DataDefinition,
        };
        let (parent_specs, parent_suggestion, parent_value, parent_type) =
            lookup_parent_type(&scope, parent)?;

        let mut declared_suggestion = parent_suggestion;
        let mut declared_value = parent_value;
        let mut final_type = (*parent_type).clone();
        final_type.specifications = parent_specs;
        if !should_defer_ranged_constraints(parent) {
            if let Some(constraints) = &constraints {
                let resolve_unit = UnitTokenResolver::Search(
                    already_resolved
                        .iter()
                        .map(|(_, _, rts)| (&rts.unit_index, &rts.resolved))
                        .collect(),
                );
                apply_constraints_to_spec(
                    spec,
                    &constraint_application_type_name(parent, type_name),
                    type_name,
                    &mut final_type,
                    constraints,
                    &ftd.source,
                    ConstraintSuggestionSink {
                        suggestion: &mut declared_suggestion,
                        value: &mut declared_value,
                        resolve_unit: Some(&resolve_unit),
                    },
                )?;
            }
        }
        let final_specs = final_type.specifications;
        let final_options = final_type.options;

        if matches!(&final_specs, TypeSpecification::List { .. }) && declared_value.is_none() {
            declared_value = Some(RawSuggestion::List(Vec::new()));
        }

        let this_spec = SpecVersionKey::of(self.registered_repository(spec), spec);
        let family = match (element_parent_type(parent), parent_type.family()) {
            (ParentType::Primitive { .. }, _) | (_, None) => {
                TypeFamily::declared_in(&this_spec, type_name)
            }
            (_, Some(parent_family)) => parent_family.clone(),
        };
        let defining_spec = match element_parent_type(parent) {
            ParentType::Qualified { .. } => TypeDefiningSpec::Import,
            _ => TypeDefiningSpec::Local,
        };
        let extends = TypeExtends::Custom {
            parent: parent.to_string(),
            family,
            defining_spec,
        };

        let lemma_type = Arc::new(LemmaType {
            name: Some(type_name.to_string()),
            specifications: final_specs,
            extends,
            options: final_options,
        });

        let declared_suggestion = match &ftd.bound_literal {
            Some(literal) => {
                let resolve_unit = UnitTokenResolver::Search(
                    already_resolved
                        .iter()
                        .map(|(_, _, rts)| (&rts.unit_index, &rts.resolved))
                        .collect(),
                );
                match semantics::parser_value_to_value_kind(
                    literal,
                    lemma_type.as_ref(),
                    Some(&resolve_unit),
                ) {
                    Ok(value_kind) => Some(RawSuggestion::Value(value_kind)),
                    Err(message) => {
                        return Err(vec![Error::validation_with_context(
                            message,
                            Some(ftd.source.clone()),
                            None::<String>,
                            Some(spec),
                            None,
                        )]);
                    }
                }
            }
            None => declared_suggestion,
        };

        Ok((lemma_type, declared_suggestion, declared_value))
    }

    /// Resolve the spec's scalar types. Returns them with the topo-sorted list type names,
    /// which [`Self::resolve_list_types`] resolves once the scalar passes are done.
    fn resolve_types_internal(
        &self,
        spec: &LemmaSpec,
        at: &EffectiveDate,
        already_resolved: &ResolvedTypesMap,
    ) -> Result<(ResolvedSpecTypes, Vec<String>), Vec<Error>> {
        let data_defs = self
            .data_types
            .iter()
            .find(|(s, _)| discovery::same_loaded_spec(s, spec))
            .map(|(_, defs)| defs)
            .cloned()
            .unwrap_or_default();

        let (scalar_names, list_names) = partition_list_type_names(&data_defs);
        let sorted_type_names = topo_sort_type_names(spec, &data_defs, scalar_names)?;
        let list_type_names = topo_sort_type_names(spec, &data_defs, list_names)?;

        let mut resolved: HashMap<String, ResolvedTypeEntry> = HashMap::new();

        for type_name in &sorted_type_names {
            let ftd = data_defs
                .get(type_name.as_str())
                .expect("BUG: topo-sorted type missing from registry");
            let entry =
                self.resolve_type_def(spec, type_name, ftd, at, already_resolved, &resolved)?;
            resolved.insert(type_name.clone(), entry);
        }

        let mut unit_index = PlanningUnitIndex::new();
        let mut errors = Vec::new();
        for unit in Self::extract_units_from_type(&semantics::primitive_ratio_arc().specifications)
        {
            unit_index.insert_owner(unit, PlanningOwner::PrimitiveRatio);
        }

        let locals: HashMap<String, Arc<LemmaType>> = resolved
            .iter()
            .map(|(name, (type_arc, _, _))| (name.clone(), Arc::clone(type_arc)))
            .collect();

        for type_name in &sorted_type_names {
            let (type_arc, _, _) = resolved
                .get(type_name)
                .expect("BUG: type was resolved but missing from map");
            let data_type_def = data_defs
                .get(type_name.as_str())
                .expect("BUG: type was resolved but not in registry");
            let merge_result = if type_arc.is_measure() {
                if let ParentType::Qualified { spec_alias, inner } = &data_type_def.parent {
                    match Self::parent_units_for_qualified_extension(
                        self,
                        spec,
                        data_type_def,
                        already_resolved,
                        at,
                        spec_alias,
                        inner,
                    ) {
                        Ok(None) => Ok(()),
                        Ok(Some(parent_units)) => Self::merge_additive_measure_units(
                            spec,
                            &mut unit_index,
                            type_arc,
                            data_type_def,
                            &[],
                            &parent_units,
                            &locals,
                        ),
                        Err(error) => Err(error),
                    }
                } else {
                    Self::add_measure_units_to_index(
                        spec,
                        &mut unit_index,
                        type_arc,
                        data_type_def,
                        &[],
                        &locals,
                    )
                }
            } else if type_arc.is_ratio() {
                Self::add_ratio_units_to_index(
                    spec,
                    &mut unit_index,
                    type_arc,
                    data_type_def,
                    &[],
                    &locals,
                )
            } else {
                Ok(())
            };
            if let Err(error) = merge_result {
                errors.push(error);
            }
        }

        for data_row in &spec.data {
            let import_alias = data_row.reference.name.clone();
            let mut merge_imported_units =
                |import_repository: &Arc<LemmaRepository>,
                 imported_spec: &LemmaSpec,
                 imported_resolved: &ResolvedSpecTypes,
                 prefix_scale_hop: bool| {
                    // A `uses` alias hop: every dynamic scale reference gains it, so a
                    // unit read here names this spec's instance. An instance-list name
                    // is not a uses hop — row factors stay in the row template's
                    // coordinates and settle in the row context.
                    let scale_hop = prefix_scale_hop.then(|| {
                        let identity = SpecIdentity::of(
                            import_repository,
                            imported_spec,
                            imported_spec.effective_from.clone(),
                        );
                        PathSegment::import(&import_alias, &identity)
                    });
                    for (bare, owner) in imported_resolved.unit_index.iter_owners() {
                        let planning_owner = match owner {
                            UnitOwner::PrimitiveRatio => PlanningOwner::PrimitiveRatio,
                            UnitOwner::Named {
                                import_path,
                                type_name,
                                lemma_type,
                            } => {
                                let mut path = vec![import_alias.clone()];
                                path.extend(import_path.iter().cloned());
                                let lemma_type = match &scale_hop {
                                    Some(hop) => Arc::new(lemma_type.with_scale_import_hop(hop)),
                                    None => Arc::clone(lemma_type),
                                };
                                PlanningOwner::Imported {
                                    import_path: path,
                                    type_name: type_name.clone(),
                                    lemma_type,
                                }
                            }
                        };
                        unit_index.insert_owner(bare.to_string(), planning_owner);
                    }
                };
            match &data_row.value {
                ParsedDataValue::Import {
                    spec_ref,
                    effective_ref: Some(_),
                    ..
                } => {
                    // Dynamic pin: merge units from every candidate already type-resolved.
                    let resolved = match discovery::resolve_spec_ref_repository(
                        self.context,
                        spec_ref,
                        Some(&data_row.source_location),
                        spec.name.as_str(),
                        Some(spec),
                    ) {
                        Ok(resolved) => resolved,
                        Err(error) => {
                            errors.push(error);
                            continue;
                        }
                    };
                    let repository = match &resolved.repository {
                        Some(explicit) => Arc::clone(explicit),
                        None => Arc::clone(self.registered_repository(spec)),
                    };
                    let dep_name =
                        crate::parsing::ast::ascii_lowercase_logical_name(spec_ref.name.clone());
                    for (repo, imported, imported_resolved) in already_resolved {
                        if !Arc::ptr_eq(repo, &repository) {
                            continue;
                        }
                        if crate::parsing::ast::ascii_lowercase_logical_name(imported.name.clone())
                            != dep_name
                        {
                            continue;
                        }
                        if discovery::same_loaded_spec(imported, spec) {
                            continue;
                        }
                        merge_imported_units(repo, imported, imported_resolved, true);
                    }
                }
                ParsedDataValue::Import { .. } => {
                    let Some(spec_ref) = data_row.value.uses_spec_ref() else {
                        continue;
                    };
                    let (import_repository, imported_spec) = match self.resolve_spec_for_import(
                        spec,
                        spec_ref,
                        &data_row.source_location,
                        at,
                    ) {
                        Ok(pair) => pair,
                        Err(error) => {
                            errors.push(error);
                            continue;
                        }
                    };
                    if discovery::same_loaded_spec(imported_spec, spec) {
                        continue;
                    }
                    let Some(imported_resolved) = already_resolved
                        .iter()
                        .find(|(_, imported, _)| {
                            discovery::same_loaded_spec(imported, imported_spec)
                        })
                        .map(|(_, _, resolved_spec_types)| resolved_spec_types)
                    else {
                        continue;
                    };
                    merge_imported_units(
                        &import_repository,
                        imported_spec,
                        imported_resolved,
                        true,
                    );
                }
                ParsedDataValue::Definition { .. } => {
                    let Some(spec_ref) = discovery::data_instance_list_spec_ref(spec, data_row)
                    else {
                        continue;
                    };
                    let ParsedDataValue::Definition {
                        base: Some(parent), ..
                    } = &data_row.value
                    else {
                        continue;
                    };
                    if Self::list_of_spec_name(spec, parent).is_none() {
                        continue;
                    }
                    let (import_repository, imported_spec) = match self
                        .resolve_list_spec_for_import(
                            spec,
                            &spec_ref,
                            &data_row.source_location,
                            at,
                        ) {
                        Ok(pair) => pair,
                        Err(error) => {
                            errors.push(error);
                            continue;
                        }
                    };
                    if discovery::same_loaded_spec(imported_spec, spec) {
                        continue;
                    }
                    let Some(imported_resolved) = already_resolved
                        .iter()
                        .find(|(_, imported, _)| {
                            discovery::same_loaded_spec(imported, imported_spec)
                        })
                        .map(|(_, _, resolved_spec_types)| resolved_spec_types)
                    else {
                        continue;
                    };
                    merge_imported_units(
                        &import_repository,
                        imported_spec,
                        imported_resolved,
                        false,
                    );
                }
                ParsedDataValue::ImportList { .. } => {
                    panic!(
                        "BUG: uses list is rejected at parse and must not reach type resolution"
                    );
                }
            }
        }

        if !errors.is_empty() {
            return Err(errors);
        }

        let mut raw_suggestions = Vec::new();
        let mut raw_values = Vec::new();
        let mut resolved_types = IndexMap::new();
        for type_name in &sorted_type_names {
            let (lemma_type, default, fill) = resolved
                .remove(type_name)
                .expect("BUG: every sorted type name must exist in resolved");
            if let Some(raw_suggestion) = default {
                raw_suggestions.push((type_name.clone(), raw_suggestion));
            }
            if let Some(raw_value) = fill {
                raw_values.push((type_name.clone(), raw_value));
            }
            resolved_types.insert(type_name.clone(), lemma_type);
        }
        if !resolved.is_empty() {
            panic!(
                "BUG: resolved types remain after draining sorted_type_names: {:?}",
                resolved.keys().collect::<Vec<_>>()
            );
        }

        Ok((
            ResolvedSpecTypes {
                resolved: resolved_types,
                declared_suggestions: IndexMap::new(),
                declared_values: IndexMap::new(),
                raw_suggestions,
                raw_values,
                source_defaults: IndexMap::new(),
                source_value_defaults: IndexMap::new(),
                rule_list_types: Vec::new(),
                unit_index: UnitIndex::new(),
                planning_unit_index: unit_index,
                signature_index: crate::computation::arithmetic::SignatureIndex::new(),
                deferred_signature_collisions: Vec::new(),
            },
            list_type_names,
        ))
    }

    fn resolve_spec_for_import(
        &self,
        spec: &LemmaSpec,
        from: &crate::parsing::ast::SpecRef,
        import_site: &crate::parsing::source::Source,
        at: &EffectiveDate,
    ) -> Result<(Arc<LemmaRepository>, &'a LemmaSpec), Error> {
        discovery::resolve_spec_ref(
            self.context,
            from,
            self.registered_repository(spec),
            spec,
            at,
            Some(import_site.clone()),
        )
    }

    /// [`Self::resolve_spec_for_import`] for `data x: s list`, where `s` may be `spec` itself.
    fn resolve_list_spec_for_import(
        &self,
        spec: &LemmaSpec,
        from: &crate::parsing::ast::SpecRef,
        import_site: &crate::parsing::source::Source,
        at: &EffectiveDate,
    ) -> Result<(Arc<LemmaRepository>, &'a LemmaSpec), Error> {
        discovery::resolve_spec_ref_or_self(
            self.context,
            from,
            self.registered_repository(spec),
            spec,
            at,
            Some(import_site.clone()),
        )
    }

    // =========================================================================
    // Static helpers (no &self)
    // =========================================================================

    fn add_measure_units_to_index(
        spec: &LemmaSpec,
        unit_index: &mut PlanningUnitIndex,
        resolved_type: &Arc<LemmaType>,
        defined_by: &DataTypeDef,
        import_path: &[String],
        locals: &HashMap<String, Arc<LemmaType>>,
    ) -> Result<(), Error> {
        if matches!(defined_by.parent, ParentType::Qualified { .. }) {
            unreachable!(
                "BUG: Qualified parents must use parent_units_for_qualified_extension + merge_additive_measure_units"
            );
        }
        let measure_family = resolved_type
            .measure_family_name()
            .expect("BUG: add_measure_units_to_index requires measure type with family");
        for unit in Self::extract_units_from_type(&resolved_type.specifications) {
            unit_index
                .merge_measure_unit(
                    unit,
                    resolved_type,
                    &defined_by.name,
                    import_path,
                    measure_family,
                    locals,
                )
                .map_err(|conflict| {
                    Self::unit_merge_conflict_to_error(conflict, defined_by, spec)
                })?;
        }
        Ok(())
    }

    /// Parent unit names for a Qualified extension; `None` if parent not resolved yet.
    fn parent_units_for_qualified_extension(
        resolver: &TypeResolver<'_>,
        spec: &LemmaSpec,
        defined_by: &DataTypeDef,
        already_resolved: &ResolvedTypesMap<'_>,
        at: &EffectiveDate,
        spec_alias: &str,
        inner: &ParentType,
    ) -> Result<Option<BTreeSet<String>>, Error> {
        let ParentType::Custom {
            name: parent_type_name,
            ..
        } = inner
        else {
            return Ok(None);
        };
        let Some(spec_ref) = spec.data.iter().find_map(|row| {
            if row.reference.segments.is_empty() && row.reference.name == spec_alias {
                row.value.uses_spec_ref()
            } else {
                None
            }
        }) else {
            return Err(Error::validation_with_context(
                format!("'{spec_alias}' is not a `uses` import alias in this spec"),
                Some(defined_by.source.clone()),
                Some(format!(
                    "Add `uses {spec_alias}: <target_spec>`, then write `{spec_alias}.<type>`."
                )),
                Some(spec),
                None,
            ));
        };
        let (_, target_spec) =
            resolver.resolve_spec_for_import(spec, spec_ref, &defined_by.source, at)?;
        let Some(imported_resolved) = already_resolved
            .iter()
            .find(|(_, imported, _)| discovery::same_loaded_spec(imported, target_spec))
            .map(|(_, _, types)| types)
        else {
            return Ok(None);
        };
        let Some(parent_type) = imported_resolved.resolved.get(parent_type_name.as_str()) else {
            return Ok(None);
        };
        Ok(Some(
            Self::extract_units_from_type(&parent_type.specifications)
                .into_iter()
                .collect(),
        ))
    }

    /// Register only units the Qualified extension newly declares (not inherited from parent).
    fn merge_additive_measure_units(
        spec: &LemmaSpec,
        unit_index: &mut PlanningUnitIndex,
        resolved_type: &Arc<LemmaType>,
        defined_by: &DataTypeDef,
        import_path: &[String],
        parent_units: &BTreeSet<String>,
        locals: &HashMap<String, Arc<LemmaType>>,
    ) -> Result<(), Error> {
        let measure_family = resolved_type
            .measure_family_name()
            .expect("BUG: additive measure registration requires family");
        for unit in Self::extract_units_from_type(&resolved_type.specifications) {
            if parent_units.contains(&unit) {
                continue;
            }
            unit_index
                .merge_measure_unit(
                    unit,
                    resolved_type,
                    &defined_by.name,
                    import_path,
                    measure_family,
                    locals,
                )
                .map_err(|conflict| {
                    Self::unit_merge_conflict_to_error(conflict, defined_by, spec)
                })?;
        }
        Ok(())
    }

    fn add_ratio_units_to_index(
        spec: &LemmaSpec,
        unit_index: &mut PlanningUnitIndex,
        resolved_type: &Arc<LemmaType>,
        defined_by: &DataTypeDef,
        import_path: &[String],
        locals: &HashMap<String, Arc<LemmaType>>,
    ) -> Result<(), Error> {
        for unit in Self::extract_units_from_type(&resolved_type.specifications) {
            unit_index
                .merge_ratio_unit(unit, resolved_type, &defined_by.name, import_path, locals)
                .map_err(|conflict| {
                    Self::unit_merge_conflict_to_error(conflict, defined_by, spec)
                })?;
        }
        Ok(())
    }

    fn unit_merge_conflict_to_error(
        conflict: UnitMergeConflict,
        defined_by: &DataTypeDef,
        spec: &LemmaSpec,
    ) -> Error {
        let message = match conflict {
            UnitMergeConflict::ConflictingFactors { unit, family } => format!(
                "Unit '{unit}' has different values on related measure types in '{family}'. \
                 Use the same unit value on each type, or use different unit names."
            ),
        };
        Error::validation_with_context(
            message,
            Some(defined_by.source.clone()),
            None::<String>,
            Some(spec),
            None,
        )
    }

    fn extract_units_from_type(specs: &TypeSpecification) -> Vec<String> {
        match specs {
            TypeSpecification::Measure { units, .. } => {
                units.iter().map(|unit| unit.name.clone()).collect()
            }
            TypeSpecification::Ratio { units, .. } => {
                units.iter().map(|unit| unit.name.clone()).collect()
            }
            _ => Vec::new(),
        }
    }
}

// ============================================================================
// Validation (formerly validation.rs)
// ============================================================================

pub(crate) fn instance_list_option_errors(
    lemma_type: &LemmaType,
    raw: &RawSuggestion,
    source: &Source,
    spec: &LemmaSpec,
) -> Vec<Error> {
    let owner = match &lemma_type.specifications {
        TypeSpecification::List { element, .. } => element.as_ref(),
        _ => lemma_type,
    };
    let mut values = Vec::new();
    let mut errors = Vec::new();
    if let Err(message) = collect_raw_values(raw, &mut values) {
        errors.push(Error::validation_with_context(
            message,
            Some(source.clone()),
            None::<String>,
            Some(spec),
            None,
        ));
        return errors;
    }
    for value in values {
        if let Err(message) = owner.reject_disallowed(&value, None) {
            errors.push(Error::validation_with_context(
                message,
                Some(source.clone()),
                None::<String>,
                Some(spec),
                None,
            ));
        }
    }
    errors
}

fn collect_raw_values(raw: &RawSuggestion, out: &mut Vec<ValueKind>) -> Result<(), String> {
    let mut pending = vec![raw];
    while let Some(current) = pending.pop() {
        match current {
            RawSuggestion::Value(value) => out.push(value.clone()),
            RawSuggestion::List(rows) => {
                for row in rows.iter().rev() {
                    pending.push(row);
                }
            }
            RawSuggestion::Measure { .. } | RawSuggestion::UnitBound { .. } => {
                return Err(
                    "an instance-list option row is a value, not a measure bound still awaiting a unit"
                        .to_string(),
                );
            }
        }
    }
    Ok(())
}

pub(crate) fn validate_type_options(
    lemma_type: &LemmaType,
    declared_default: Option<&BoundValueKind>,
    default_label: &str,
    type_name: &str,
    source: &Source,
    spec_context: Option<&LemmaSpec>,
    unit_index: &UnitIndex,
) -> Vec<Error> {
    let owner = match &lemma_type.specifications {
        TypeSpecification::List { element, .. } if !element.options.is_empty() => element.as_ref(),
        _ => lemma_type,
    };
    let mut errors = Vec::new();
    let mut bare = owner.clone();
    bare.options = TypeOptions::default();
    for option in &owner.options.values {
        let literal = LiteralValue {
            value: option.value.value.clone(),
        };
        if let Err(message) =
            execution_plan::validate_value_against_type(&bare, &literal, unit_index, None)
        {
            errors.push(Error::validation_with_context(
                format!(
                    "Type '{type_name}' option '{}': {message}",
                    owner.display_bound(&option.value)
                ),
                Some(source.clone()),
                None::<String>,
                spec_context,
                None,
            ));
        }
    }
    if let Some(default) = declared_default {
        if let Err(message) = execution_plan::validate_value_against_type(
            lemma_type,
            &default.to_literal(),
            unit_index,
            None,
        ) {
            errors.push(Error::validation_with_context(
                format!("Type '{type_name}' {default_label} value {message}"),
                Some(source.clone()),
                None::<String>,
                spec_context,
                None,
            ));
        }
    }
    errors
}

/// Validate that TypeSpecification constraints are internally consistent.
///
/// Checks range, decimals, length, unit, and option constraints, and
/// validates the declared default (when present) against those constraints.
/// The default lives outside the type specification (on the data binding or
/// typedef entry); callers thread it in explicitly so this function can verify
/// consistency without owning the value. Pass `default_label` as `"suggestion"`
/// or `"with"` for error messages.
///
/// Returns a vector of errors (empty if valid).
pub fn validate_type_specifications(
    specs: &TypeSpecification,
    declared_default: Option<&BoundValueKind>,
    default_label: &str,
    type_name: &str,
    source: &Source,
    spec_context: Option<&LemmaSpec>,
    unit_index: &UnitIndex,
) -> Vec<Error> {
    let mut errors = Vec::new();
    let declared_default = declared_default.map(|bound| &bound.value);

    match specs {
        TypeSpecification::Measure {
            minimum,
            maximum,
            units,
            ..
        } => {
            // Validate range consistency
            if let (Some(min), Some(max)) = (minimum, maximum) {
                match (
                    semantics::measure_declared_bound_to_canonical(
                        &min.0, &min.1, units, type_name, "minimum",
                    ),
                    semantics::measure_declared_bound_to_canonical(
                        &max.0, &max.1, units, type_name, "maximum",
                    ),
                ) {
                    (Ok(min_canonical), Ok(max_canonical)) => {
                        match min_canonical.try_cmp(&max_canonical) {
                            Ok(std::cmp::Ordering::Greater) => {
                                errors.push(Error::validation_with_context(
                                    format!(
                                        "Type '{}' has invalid range: minimum {} {} is greater than maximum {} {}",
                                        type_name,
                                        min.0,
                                        min.1,
                                        max.0,
                                        max.1
                                    ),
                                    Some(source.clone()),
                                    None::<String>,
                                    spec_context,
                                    None,
                                ));
                            }
                            Ok(std::cmp::Ordering::Less | std::cmp::Ordering::Equal) => {}
                            Err(failure) => {
                                errors.push(Error::validation_with_context(
                                    format!(
                                        "Type '{}' has invalid measure bound: range compare failed: {failure}",
                                        type_name
                                    ),
                                    Some(source.clone()),
                                    None::<String>,
                                    spec_context,
                                    None,
                                ));
                            }
                        }
                    }
                    (Err(message), _) | (_, Err(message)) => {
                        errors.push(Error::validation_with_context(
                            format!(
                                "Type '{}' has invalid measure bound: {}",
                                type_name, message
                            ),
                            Some(source.clone()),
                            None::<String>,
                            spec_context,
                            None,
                        ));
                    }
                }
            }

            // Measure defaults are validated during suggestion lift against this type's units.

            // Measure types must have at least one unit (required for parsing and conversion)
            if units.is_empty() {
                errors.push(Error::validation_with_context(
                    format!(
                        "Type '{}' is a measure type but has no units. Measure types must define at least one unit (e.g. -> unit eur: 1).",
                        type_name
                    ),
                    Some(source.clone()),
                    None::<String>,
                    spec_context,
                    None,
                ));
            }

            // Validate units (if present)
            if !units.is_empty() {
                let mut seen_names: Vec<String> = Vec::new();
                for unit in units.iter() {
                    // Validate unit name is not empty
                    if unit.name.trim().is_empty() {
                        errors.push(Error::validation_with_context(
                            format!(
                                "Type '{}' has a unit with empty name. Unit names cannot be empty.",
                                type_name
                            ),
                            Some(source.clone()),
                            None::<String>,
                            spec_context,
                            None,
                        ));
                    }

                    // Validate unit names are unique within the type
                    if seen_names.contains(&unit.name) {
                        errors.push(Error::validation_with_context(
                            format!("Type '{}' has duplicate unit name '{}'. Unit names must be unique within a type.", type_name, unit.name),
                            Some(source.clone()),
                            None::<String>,
                            spec_context,
                            None,
                        ));
                    } else {
                        seen_names.push(unit.name.clone());
                    }

                    if !unit.is_positive_factor() {
                        let factor = unit.require_literal_factor();
                        errors.push(Error::validation_with_context(
                            format!("Type '{}' has unit '{}' with invalid value {}/{}. Unit values must be positive (conversion factor relative to type base).", type_name, unit.name, factor.numer_to_string(), factor.denom_to_string()),
                            Some(source.clone()),
                            None::<String>,
                            spec_context,
                            None,
                        ));
                    }
                }
            }
        }
        TypeSpecification::Number {
            minimum,
            maximum,
            decimals,
            ..
        } => {
            // Validate range consistency
            if let (Some(min), Some(max)) = (minimum, maximum) {
                match min.try_cmp(max) {
                    Ok(std::cmp::Ordering::Greater) => {
                        errors.push(Error::validation_with_context(
                            format!(
                                "Type '{}' has invalid range: minimum {} is greater than maximum {}",
                                type_name, min, max
                            ),
                            Some(source.clone()),
                            None::<String>,
                            spec_context,
                            None,
                        ));
                    }
                    Ok(std::cmp::Ordering::Less | std::cmp::Ordering::Equal) => {}
                    Err(failure) => {
                        errors.push(Error::validation_with_context(
                            format!(
                                "Type '{}' has invalid range: bound compare failed: {failure}",
                                type_name
                            ),
                            Some(source.clone()),
                            None::<String>,
                            spec_context,
                            None,
                        ));
                    }
                }
            }

            // Validate decimals range (0-28 is rust_decimal limit)
            if let Some(d) = decimals {
                if *d > 28 {
                    errors.push(Error::validation_with_context(
                        format!(
                            "Type '{}' has invalid decimals value: {}. Must be between 0 and 28",
                            type_name, d
                        ),
                        Some(source.clone()),
                        None::<String>,
                        spec_context,
                        None,
                    ));
                }
            }

            if let Some(ValueKind::Number(def)) = declared_default {
                if let Some(min) = minimum {
                    match def.try_cmp(min) {
                        Ok(std::cmp::Ordering::Less) => {
                            errors.push(Error::validation_with_context(
                                format!(
                                    "Type '{}' {default_label} value {} is less than minimum {}",
                                    type_name, def, min
                                ),
                                Some(source.clone()),
                                None::<String>,
                                spec_context,
                                None,
                            ));
                        }
                        Ok(std::cmp::Ordering::Equal | std::cmp::Ordering::Greater) => {}
                        Err(failure) => {
                            errors.push(Error::validation_with_context(
                                format!(
                                    "Type '{}' {default_label} bound compare failed: {failure}",
                                    type_name
                                ),
                                Some(source.clone()),
                                None::<String>,
                                spec_context,
                                None,
                            ));
                        }
                    }
                }
                if let Some(max) = maximum {
                    match def.try_cmp(max) {
                        Ok(std::cmp::Ordering::Greater) => {
                            errors.push(Error::validation_with_context(
                                format!(
                                    "Type '{}' {default_label} value {} is greater than maximum {}",
                                    type_name, def, max
                                ),
                                Some(source.clone()),
                                None::<String>,
                                spec_context,
                                None,
                            ));
                        }
                        Ok(std::cmp::Ordering::Less | std::cmp::Ordering::Equal) => {}
                        Err(failure) => {
                            errors.push(Error::validation_with_context(
                                format!(
                                    "Type '{}' {default_label} bound compare failed: {failure}",
                                    type_name
                                ),
                                Some(source.clone()),
                                None::<String>,
                                spec_context,
                                None,
                            ));
                        }
                    }
                }
            }
            // Note: Number types are dimensionless and cannot have units (validated in apply_constraint)
        }

        TypeSpecification::Ratio {
            minimum,
            maximum,
            units,
            ..
        } => {
            // Validate range consistency
            if let (Some(min), Some(max)) = (minimum, maximum) {
                match min.try_cmp(max) {
                    Ok(std::cmp::Ordering::Greater) => {
                        errors.push(Error::validation_with_context(
                            format!(
                                "Type '{}' has invalid range: minimum {} is greater than maximum {}",
                                type_name, min, max
                            ),
                            Some(source.clone()),
                            None::<String>,
                            spec_context,
                            None,
                        ));
                    }
                    Ok(std::cmp::Ordering::Less | std::cmp::Ordering::Equal) => {}
                    Err(failure) => {
                        errors.push(Error::validation_with_context(
                            format!(
                                "Type '{}' has invalid range: bound compare failed: {failure}",
                                type_name
                            ),
                            Some(source.clone()),
                            None::<String>,
                            spec_context,
                            None,
                        ));
                    }
                }
            }

            if let Some(ValueKind::Ratio(def)) = declared_default {
                if let Some(min) = minimum {
                    match def.try_cmp(min) {
                        Ok(std::cmp::Ordering::Less) => {
                            errors.push(Error::validation_with_context(
                                format!(
                                    "Type '{}' {default_label} value {} is less than minimum {}",
                                    type_name, def, min
                                ),
                                Some(source.clone()),
                                None::<String>,
                                spec_context,
                                None,
                            ));
                        }
                        Ok(std::cmp::Ordering::Equal | std::cmp::Ordering::Greater) => {}
                        Err(failure) => {
                            errors.push(Error::validation_with_context(
                                format!(
                                    "Type '{}' {default_label} bound compare failed: {failure}",
                                    type_name
                                ),
                                Some(source.clone()),
                                None::<String>,
                                spec_context,
                                None,
                            ));
                        }
                    }
                }
                if let Some(max) = maximum {
                    match def.try_cmp(max) {
                        Ok(std::cmp::Ordering::Greater) => {
                            errors.push(Error::validation_with_context(
                                format!(
                                    "Type '{}' {default_label} value {} is greater than maximum {}",
                                    type_name, def, max
                                ),
                                Some(source.clone()),
                                None::<String>,
                                spec_context,
                                None,
                            ));
                        }
                        Ok(std::cmp::Ordering::Less | std::cmp::Ordering::Equal) => {}
                        Err(failure) => {
                            errors.push(Error::validation_with_context(
                                format!(
                                    "Type '{}' {default_label} bound compare failed: {failure}",
                                    type_name
                                ),
                                Some(source.clone()),
                                None::<String>,
                                spec_context,
                                None,
                            ));
                        }
                    }
                }
            }

            // Validate units (if present)
            // Types can have zero units (e.g., type ratio: number -> ratio) - this is valid
            // Only validate if units are defined
            if !units.is_empty() {
                let mut seen_names: Vec<String> = Vec::new();
                for unit in units.iter() {
                    // Validate unit name is not empty
                    if unit.name.trim().is_empty() {
                        errors.push(Error::validation_with_context(
                            format!(
                                "Type '{}' has a unit with empty name. Unit names cannot be empty.",
                                type_name
                            ),
                            Some(source.clone()),
                            None::<String>,
                            spec_context,
                            None,
                        ));
                    }

                    // Validate unit names are unique within the type
                    if seen_names.contains(&unit.name) {
                        errors.push(Error::validation_with_context(
                            format!("Type '{}' has duplicate unit name '{}'. Unit names must be unique within a type.", type_name, unit.name),
                            Some(source.clone()),
                            None::<String>,
                            spec_context,
                            None,
                        ));
                    } else {
                        seen_names.push(unit.name.clone());
                    }

                    if !unit.value.numer_is_positive() {
                        errors.push(Error::validation_with_context(
                            format!("Type '{}' has unit '{}' with invalid value {}/{}. Unit values must be positive (conversion factor relative to type base).", type_name, unit.name, unit.value.numer_to_string(), unit.value.denom_to_string()),
                            Some(source.clone()),
                            None::<String>,
                            spec_context,
                            None,
                        ));
                    }
                }
            }
        }

        TypeSpecification::Text { length, .. } => {
            if let Some(ValueKind::Text(def)) = declared_default {
                let def_len = def.len();

                if let Some(len) = length {
                    if def_len != *len {
                        errors.push(Error::validation_with_context(
                            format!("Type '{}' {default_label} value length {} does not match required length {}", type_name, def_len, len),
                            Some(source.clone()),
                            None::<String>,
                            spec_context,
                            None,
                        ));
                    }
                }
            }
        }

        TypeSpecification::Date {
            minimum, maximum, ..
        } => {
            // Validate range consistency
            if let (Some(min), Some(max)) = (minimum, maximum) {
                let min_sem = semantics::date_time_to_semantic(min);
                let max_sem = semantics::date_time_to_semantic(max);
                if semantics::compare_semantic_dates(&min_sem, &max_sem) == Ordering::Greater {
                    errors.push(Error::validation_with_context(
                        format!(
                            "Type '{}' has invalid date range: minimum {} is after maximum {}",
                            type_name, min, max
                        ),
                        Some(source.clone()),
                        None::<String>,
                        spec_context,
                        None,
                    ));
                }
            }

            if let Some(ValueKind::Date(def)) = declared_default {
                if let Some(min) = minimum {
                    let min_sem = semantics::date_time_to_semantic(min);
                    if semantics::compare_semantic_dates(def, &min_sem) == Ordering::Less {
                        errors.push(Error::validation_with_context(
                            format!(
                                "Type '{}' default date {} is before minimum {}",
                                type_name, def, min
                            ),
                            Some(source.clone()),
                            None::<String>,
                            spec_context,
                            None,
                        ));
                    }
                }
                if let Some(max) = maximum {
                    let max_sem = semantics::date_time_to_semantic(max);
                    if semantics::compare_semantic_dates(def, &max_sem) == Ordering::Greater {
                        errors.push(Error::validation_with_context(
                            format!(
                                "Type '{}' default date {} is after maximum {}",
                                type_name, def, max
                            ),
                            Some(source.clone()),
                            None::<String>,
                            spec_context,
                            None,
                        ));
                    }
                }
            }
        }

        TypeSpecification::Time {
            minimum, maximum, ..
        } => {
            // Validate range consistency
            if let (Some(min), Some(max)) = (minimum, maximum) {
                let min_sem = semantics::time_to_semantic(min);
                let max_sem = semantics::time_to_semantic(max);
                if semantics::compare_semantic_times(&min_sem, &max_sem) == Ordering::Greater {
                    errors.push(Error::validation_with_context(
                        format!(
                            "Type '{}' has invalid time range: minimum {} is after maximum {}",
                            type_name, min, max
                        ),
                        Some(source.clone()),
                        None::<String>,
                        spec_context,
                        None,
                    ));
                }
            }

            if let Some(ValueKind::Time(def)) = declared_default {
                if let Some(min) = minimum {
                    let min_sem = semantics::time_to_semantic(min);
                    if semantics::compare_semantic_times(def, &min_sem) == Ordering::Less {
                        errors.push(Error::validation_with_context(
                            format!(
                                "Type '{}' default time {} is before minimum {}",
                                type_name, def, min
                            ),
                            Some(source.clone()),
                            None::<String>,
                            spec_context,
                            None,
                        ));
                    }
                }
                if let Some(max) = maximum {
                    let max_sem = semantics::time_to_semantic(max);
                    if semantics::compare_semantic_times(def, &max_sem) == Ordering::Greater {
                        errors.push(Error::validation_with_context(
                            format!(
                                "Type '{}' default time {} is after maximum {}",
                                type_name, def, max
                            ),
                            Some(source.clone()),
                            None::<String>,
                            spec_context,
                            None,
                        ));
                    }
                }
            }
        }

        TypeSpecification::NumberRange { .. }
        | TypeSpecification::DateRange { .. }
        | TypeSpecification::TimeRange { .. }
        | TypeSpecification::MeasureRange { .. }
        | TypeSpecification::RatioRange { .. } => {
            if let Err(message) = semantics::check_range_bound_consistency(specs, unit_index) {
                errors.push(Error::validation_with_context(
                    format!("Type '{}' has {message}", type_name),
                    Some(source.clone()),
                    None::<String>,
                    spec_context,
                    None,
                ));
            }
        }
        TypeSpecification::Boolean { .. } => {
            // No constraint validation needed for these types
        }
        TypeSpecification::Veto { .. } => {
            // Veto is not a user-declarable type, so validation should not be called on it
            // But if it is, there's nothing to validate
        }
        TypeSpecification::Undetermined => unreachable!(
            "BUG: validate_type_specification_constraints called with Undetermined sentinel type; this type exists only during type inference"
        ),
        TypeSpecification::Spec { .. } | TypeSpecification::List { .. } => {}
    }

    errors
}

#[cfg(test)]
mod tests {
    use super::*;

    mod type_resolution {
        use super::*;
        use crate::computation::rational::rational_new;
        use crate::parsing::ast::{
            CommandArg, LemmaSpec, ParentType, PrimitiveKind, TypeConstraintCommand,
        };
        use crate::parsing::parse;
        use crate::planning::typing::{find_unique_measure_type_in_unit_index, DecompositionMatch};
        use crate::ResourceLimits;
        use rust_decimal::Decimal;
        use std::sync::Arc;

        fn test_context_and_effective(
            specs: &[&LemmaSpec],
        ) -> (&'static Context, &'static EffectiveDate) {
            use crate::engine::Context;
            let mut ctx = Context::new();
            let repository = ctx.workspace();
            for s in specs {
                ctx.insert_spec(Arc::clone(&repository), (*s).clone())
                    .unwrap();
            }
            let ctx = Box::leak(Box::new(ctx));
            let eff = Box::leak(Box::new(EffectiveDate::Origin));
            (ctx, eff)
        }

        fn context_with_test_spec() -> (&'static Context, &'static LemmaSpec) {
            use crate::engine::Context;
            let mut ctx = Context::new();
            let repository = ctx.workspace();
            let spec = LemmaSpec::new("test_spec".to_string());
            ctx.insert_spec(Arc::clone(&repository), spec).unwrap();
            let ctx: &'static Context = Box::leak(Box::new(ctx));
            let repository = ctx.workspace();
            let spec = ctx
                .spec_set(&repository, "test_spec")
                .and_then(|ss| ss.get_exact(None))
                .expect("inserted");
            (ctx, spec)
        }

        fn resolver_for_code(code: &str) -> (TypeResolver<'static>, Vec<&'static LemmaSpec>) {
            use crate::engine::Context;
            let owned = parse(
                code,
                crate::parsing::source::SourceType::Volatile,
                &ResourceLimits::default(),
            )
            .unwrap()
            .into_flattened_specs();
            let mut ctx = Context::new();
            let repository = ctx.workspace();
            for s in owned {
                ctx.insert_spec(Arc::clone(&repository), s).unwrap();
            }
            let ctx = Box::leak(Box::new(ctx));
            let repository = ctx.workspace();
            let mut spec_refs: Vec<&'static LemmaSpec> = Vec::new();
            for ss in ctx.spec_sets_for(&repository) {
                for s in ss.iter_specs() {
                    spec_refs.push(s);
                }
            }
            let mut resolver = TypeResolver::new(ctx, HashMap::new());
            for spec in &spec_refs {
                resolver.register_all(&repository, spec);
            }
            (resolver, spec_refs)
        }

        fn resolver_single_spec(code: &str) -> (TypeResolver<'static>, &LemmaSpec) {
            let (resolver, spec_arcs) = resolver_for_code(code);
            let spec = spec_arcs.into_iter().next().expect("at least one spec");
            (resolver, spec)
        }

        #[test]
        fn test_type_spec_for_primitive_covers_all_variants() {
            use crate::parsing::ast::PrimitiveKind;
            use crate::planning::semantics::type_spec_for_primitive;

            for kind in [
                PrimitiveKind::Boolean,
                PrimitiveKind::Measure,
                PrimitiveKind::MeasureRange,
                PrimitiveKind::Number,
                PrimitiveKind::NumberRange,
                PrimitiveKind::Ratio,
                PrimitiveKind::RatioRange,
                PrimitiveKind::Text,
                PrimitiveKind::Date,
                PrimitiveKind::DateRange,
                PrimitiveKind::Time,
                PrimitiveKind::TimeRange,
            ] {
                let spec = type_spec_for_primitive(kind);
                assert!(
                    !matches!(
                        spec,
                        crate::planning::semantics::TypeSpecification::Undetermined
                    ),
                    "type_spec_for_primitive({:?}) returned Undetermined",
                    kind
                );
            }
        }

        #[test]
        fn test_register_data_type_def() {
            let (ctx, spec) = context_with_test_spec();
            let mut resolver = TypeResolver::new(ctx, HashMap::new());
            let ftd = DataTypeDef {
                parent: ParentType::Primitive {
                    primitive: PrimitiveKind::Number,
                },
                constraints: Some(vec![
                    Constraint::new(
                        TypeConstraintCommand::Minimum,
                        vec![CommandArg::Literal(crate::literals::Value::Number(
                            Decimal::ZERO,
                        ))],
                        crate::parsing::source::Source::new(
                            crate::parsing::source::SourceType::Volatile,
                            crate::parsing::ast::Span {
                                start: 0,
                                end: 0,
                                line: 1,
                                col: 0,
                            },
                        ),
                    ),
                    Constraint::new(
                        TypeConstraintCommand::Maximum,
                        vec![CommandArg::Literal(crate::literals::Value::Number(
                            Decimal::from(150),
                        ))],
                        crate::parsing::source::Source::new(
                            crate::parsing::source::SourceType::Volatile,
                            crate::parsing::ast::Span {
                                start: 0,
                                end: 0,
                                line: 1,
                                col: 0,
                            },
                        ),
                    ),
                ]),
                source: crate::parsing::source::Source::new(
                    crate::parsing::source::SourceType::Volatile,
                    crate::parsing::ast::Span {
                        start: 0,
                        end: 0,
                        line: 1,
                        col: 0,
                    },
                ),
                name: "age".to_string(),
                bound_literal: None,
            };

            let result = resolver.register_type(&ctx.workspace(), spec, ftd);
            assert!(result.is_ok());
            let resolved = resolver
                .resolve_and_validate(spec, &EffectiveDate::Origin, &Vec::new())
                .unwrap();
            assert!(resolved.resolved.contains_key("age"));
        }

        #[test]
        fn test_register_duplicate_type_fails() {
            let (ctx, spec) = context_with_test_spec();
            let mut resolver = TypeResolver::new(ctx, HashMap::new());
            let ftd = DataTypeDef {
                parent: ParentType::Primitive {
                    primitive: PrimitiveKind::Number,
                },
                constraints: None,
                source: crate::parsing::source::Source::new(
                    crate::parsing::source::SourceType::Volatile,
                    crate::parsing::ast::Span {
                        start: 0,
                        end: 0,
                        line: 1,
                        col: 0,
                    },
                ),
                name: "money".to_string(),
                bound_literal: None,
            };
            let repository = ctx.workspace();
            resolver
                .register_type(&repository, spec, ftd.clone())
                .unwrap();
            let result = resolver.register_type(&repository, spec, ftd);
            assert!(result.is_err());
        }

        #[test]
        fn test_resolve_custom_type_from_primitive() {
            let (ctx, spec) = context_with_test_spec();
            let mut resolver = TypeResolver::new(ctx, HashMap::new());
            let ftd = DataTypeDef {
                parent: ParentType::Primitive {
                    primitive: PrimitiveKind::Number,
                },
                constraints: None,
                source: crate::parsing::source::Source::new(
                    crate::parsing::source::SourceType::Volatile,
                    crate::parsing::ast::Span {
                        start: 0,
                        end: 0,
                        line: 1,
                        col: 0,
                    },
                ),
                name: "money".to_string(),
                bound_literal: None,
            };

            resolver.register_type(&ctx.workspace(), spec, ftd).unwrap();
            let resolved = resolver
                .resolve_and_validate(spec, &EffectiveDate::Origin, &Vec::new())
                .unwrap();

            assert!(resolved.resolved.contains_key("money"));
            let money_type = resolved.resolved.get("money").unwrap();
            assert_eq!(money_type.name, Some("money".to_string()));
        }

        #[test]
        fn test_child_measure_type_keeps_declared_name_and_child_units() {
            let (resolver, spec) = resolver_single_spec(
                r#"spec test
    data length: measure
      -> unit meter: 1
    data road_length: length
      -> unit kilometer: 1000"#,
            );

            let resolved_types = resolver
                .resolve_and_validate(spec, &EffectiveDate::Origin, &Vec::new())
                .unwrap();

            let road_length_type = resolved_types.resolved.get("road_length").unwrap();
            assert_eq!(road_length_type.name.as_deref(), Some("road_length"));

            match &road_length_type.specifications {
                TypeSpecification::Measure { units, .. } => {
                    assert!(units.iter().any(|unit| unit.name == "kilometer"));
                }
                _ => panic!("Expected Measure type specifications"),
            }

            let kilometer_owner = resolved_types.unit_index.unique_owner("kilometer").unwrap();
            assert_eq!(kilometer_owner.name.as_deref(), Some("road_length"));
            assert!(
                Arc::ptr_eq(
                    kilometer_owner,
                    resolved_types.resolved.get("road_length").unwrap()
                ),
                "bound kilometer owner must be the resolved road_length Arc"
            );
        }

        #[test]
        fn test_type_definition_resolution() {
            let (resolver, spec) = resolver_single_spec(
                r#"spec test
    data dice: number -> minimum 0 -> maximum 6"#,
            );

            let resolved_types = resolver
                .resolve_and_validate(spec, &EffectiveDate::Origin, &Vec::new())
                .unwrap();
            let dice_type = resolved_types.resolved.get("dice").unwrap();

            match &dice_type.specifications {
                TypeSpecification::Number {
                    minimum, maximum, ..
                } => {
                    assert_eq!(minimum, &Some(rational_new(0, 1)));
                    assert_eq!(maximum, &Some(rational_new(6, 1)));
                }
                _ => panic!("Expected Number type specifications"),
            }
        }

        #[test]
        fn test_type_definition_with_multiple_commands() {
            let (resolver, spec) = resolver_single_spec(
                r#"spec test
    data money: measure -> unit eur: 1.0 -> unit usd: 1.18 -> minimum 0 eur"#,
            );

            let resolved_types = resolver
                .resolve_and_validate(spec, &EffectiveDate::Origin, &Vec::new())
                .unwrap();
            let money_type = resolved_types.resolved.get("money").unwrap();

            match &money_type.specifications {
                TypeSpecification::Measure { minimum, units, .. } => {
                    assert_eq!(minimum.as_ref().map(|(_, unit)| unit.as_str()), Some("eur"));
                    assert_eq!(units.len(), 2);
                    assert!(units.iter().any(|u| u.name == "eur"));
                    assert!(units.iter().any(|u| u.name == "usd"));
                }
                _ => panic!("Expected Measure type specifications"),
            }
        }

        #[test]
        fn test_number_type_with_decimals() {
            let (resolver, spec) = resolver_single_spec(
                r#"spec test
    data price: number -> decimals 2 -> minimum 0"#,
            );

            let resolved_types = resolver
                .resolve_and_validate(spec, &EffectiveDate::Origin, &Vec::new())
                .unwrap();
            let price_type = resolved_types.resolved.get("price").unwrap();

            match &price_type.specifications {
                TypeSpecification::Number {
                    decimals, minimum, ..
                } => {
                    assert_eq!(*decimals, Some(2));
                    assert_eq!(minimum, &Some(rational_new(0, 1)));
                }
                _ => panic!("Expected Number type specifications with decimals"),
            }
        }

        #[test]
        fn test_number_type_decimals_only() {
            let (resolver, spec) = resolver_single_spec(
                r#"spec test
    data precise_number: number -> decimals 4"#,
            );

            let resolved_types = resolver
                .resolve_and_validate(spec, &EffectiveDate::Origin, &Vec::new())
                .unwrap();
            let precise_type = resolved_types.resolved.get("precise_number").unwrap();

            match &precise_type.specifications {
                TypeSpecification::Number { decimals, .. } => {
                    assert_eq!(*decimals, Some(4));
                }
                _ => panic!("Expected Number type with decimals 4"),
            }
        }

        #[test]
        fn test_measure_type_rejects_decimals() {
            let (resolver, spec) = resolver_single_spec(
                r#"spec test
    data weight: measure -> unit kg: 1 -> decimals 3"#,
            );

            let errors = resolver
                .resolve_and_validate(spec, &EffectiveDate::Origin, &Vec::new())
                .expect_err("decimals on a measure must be a planning error");
            let messages: Vec<String> = errors.iter().map(|e| e.to_string()).collect();
            assert!(
                messages.iter().any(|m| m.contains(
                    "'decimals' is not valid on measure type 'weight': precision depends on the unit. Round in a rule instead, for example 'weight -> round 0.001 kg'."
                )),
                "got: {messages:?}"
            );
        }

        #[test]
        fn test_ratio_type_rejects_decimals() {
            let (resolver, spec) = resolver_single_spec(
                r#"spec test
    data ratio_type: ratio -> decimals 2"#,
            );

            let errors = resolver
                .resolve_and_validate(spec, &EffectiveDate::Origin, &Vec::new())
                .expect_err("decimals on a ratio must be a planning error");
            let messages: Vec<String> = errors.iter().map(|e| e.to_string()).collect();
            assert!(
                messages.iter().any(|m| m.contains(
                    "'decimals' is not valid on ratio type 'ratio_type': precision depends on the unit. Round in a rule instead, for example 'ratio_type -> round 0.01 percent'."
                )),
                "got: {messages:?}"
            );
        }

        #[test]
        fn typedef_default_inherits_through_extension_chain() {
            let (resolver, spec) = resolver_single_spec(
                r#"spec test
    data money: measure -> unit eur: 1 -> suggest 4 eur
    data price: money
    data final_price: price"#,
            );

            let resolved = resolver
                .resolve_and_validate(spec, &EffectiveDate::Origin, &Vec::new())
                .expect("extension chain must resolve");

            assert!(
                resolved.declared_suggestions.contains_key("money"),
                "base typedef default must convert to ValueKind"
            );
            assert!(
                resolved.declared_suggestions.contains_key("final_price"),
                "suggestion must inherit through extension chain (money → price → final_price)"
            );
        }

        #[test]
        fn test_ratio_type_with_default_command() {
            let (resolver, spec) = resolver_single_spec(
                r#"spec test
    data percentage: ratio -> minimum 0% -> maximum 100% -> suggest 50%"#,
            );

            let resolved_types = resolver
                .resolve_and_validate(spec, &EffectiveDate::Origin, &Vec::new())
                .unwrap();
            let percentage_type = resolved_types.resolved.get("percentage").unwrap();

            match &percentage_type.specifications {
                TypeSpecification::Ratio {
                    minimum, maximum, ..
                } => {
                    assert_eq!(
                        *minimum,
                        Some(rational_new(0, 1)),
                        "ratio type should have minimum 0"
                    );
                    assert_eq!(
                        *maximum,
                        Some(rational_new(1, 1)),
                        "ratio type should have maximum 1"
                    );
                }
                _ => panic!("Expected Ratio type with minimum and maximum"),
            }

            let declared = resolved_types
                .declared_suggestions
                .get("percentage")
                .expect("declared default must be tracked for percentage");
            match &declared.value {
                ValueKind::Ratio(v) => {
                    assert_eq!(v, &rational_new(1, 2));
                }
                other => panic!("expected Ratio declared default, got {:?}", other),
            }
        }

        #[test]
        fn test_measure_extension_chain_same_family_units_allowed() {
            let (resolver, spec) = resolver_single_spec(
                r#"spec test
    data money: measure -> unit eur: 1
    data money2: money -> unit usd: 1.24"#,
            );

            let result = resolver.resolve_and_validate(spec, &EffectiveDate::Origin, &Vec::new());
            assert!(
                result.is_ok(),
                "Measure extension chain should resolve: {:?}",
                result.err()
            );

            let resolved = result.unwrap();
            assert!(
                resolved.unit_index.unique_owner("eur").is_some(),
                "eur should be in unit_index"
            );
            assert!(
                resolved.unit_index.unique_owner("usd").is_some(),
                "usd should be in unit_index"
            );
            let eur_type = resolved.unit_index.unique_owner("eur").unwrap();
            let usd_type = resolved.unit_index.unique_owner("usd").unwrap();
            assert_eq!(
                eur_type.name.as_deref(),
                Some("money"),
                "declarer money must own inherited eur, not extension"
            );
            assert_eq!(
                usd_type.name.as_deref(),
                Some("money2"),
                "usd defined on money2 should be owned by money2"
            );
            assert!(
                Arc::ptr_eq(eur_type, resolved.resolved.get("money").unwrap()),
                "bound eur owner must be the resolved money Arc"
            );
            assert!(
                Arc::ptr_eq(usd_type, resolved.resolved.get("money2").unwrap()),
                "bound usd owner must be the resolved money2 Arc"
            );
        }

        #[test]
        fn test_invalid_parent_type_in_named_type_should_error() {
            let (resolver, spec) = resolver_single_spec(
                r#"spec test
    data invalid: nonexistent_type -> minimum 0"#,
            );

            let result = resolver.resolve_and_validate(spec, &EffectiveDate::Origin, &Vec::new());
            assert!(result.is_err(), "Should reject invalid parent type");

            let errs = result.unwrap_err();
            assert!(!errs.is_empty(), "expected at least one error");
            let error_msg = errs[0].to_string();
            assert!(
                error_msg.contains("Unknown parent") && error_msg.contains("nonexistent_type"),
                "Error should mention unknown type. Got: {}",
                error_msg
            );
        }

        #[test]
        fn test_invalid_primitive_type_name_should_error() {
            let (resolver, spec) = resolver_single_spec(
                r#"spec test
    data invalid: choice -> option "a""#,
            );

            let result = resolver.resolve_and_validate(spec, &EffectiveDate::Origin, &Vec::new());
            assert!(result.is_err(), "Should reject invalid type base 'choice'");

            let errs = result.unwrap_err();
            assert!(!errs.is_empty(), "expected at least one error");
            let error_msg = errs[0].to_string();
            assert!(
                error_msg.contains("Unknown parent") && error_msg.contains("choice"),
                "Error should mention unknown type 'choice'. Got: {}",
                error_msg
            );
        }

        #[test]
        fn extension_unit_factor_override_errors() {
            let (resolver, spec) = resolver_single_spec(
                r#"spec test
    data money: measure
      -> unit eur: 1
    data money2: money
      -> unit eur: 1.10"#,
            );

            let result = resolver.resolve_and_validate(spec, &EffectiveDate::Origin, &Vec::new());
            assert!(
                result.is_err(),
                "extension child must not override inherited unit factor"
            );
            let error_msg = result
                .unwrap_err()
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join("; ");
            assert!(
                error_msg.contains("eur"),
                "error must name unit eur, got: {error_msg}"
            );
            assert!(
                error_msg.contains("inherited") || error_msg.contains("cannot change"),
                "error must reject inherited unit redefinition, got: {error_msg}"
            );
        }

        #[test]
        fn inherited_unit_idempotent_redeclare_ok() {
            let (resolver, spec) = resolver_single_spec(
                r#"spec test
    data money: measure
      -> unit eur: 1
    data money2: money
      -> unit eur: 1"#,
            );

            let resolved = resolver
                .resolve_and_validate(spec, &EffectiveDate::Origin, &Vec::new())
                .expect("idempotent inherited unit redeclare must resolve");
            let money2 = resolved.resolved.get("money2").expect("money2");
            match &money2.specifications {
                TypeSpecification::Measure { units, .. } => {
                    let eur = units.iter().find(|u| u.name == "eur").expect("eur");
                    assert_eq!(
                        eur.require_literal_factor().try_to_decimal().unwrap(),
                        Decimal::ONE,
                        "eur factor must remain 1"
                    );
                }
                other => panic!("expected Measure, got {other:?}"),
            }
        }

        #[test]
        fn extension_additive_unit_registers_in_unit_index() {
            let (resolver, spec) = resolver_single_spec(
                r#"spec test
    data money: measure
      -> unit eur: 1
    data money2: money
      -> unit usd: 1.24"#,
            );

            let resolved = resolver
                .resolve_and_validate(spec, &EffectiveDate::Origin, &Vec::new())
                .expect("additive extension unit must resolve");
            let money = resolved
                .resolved
                .get("money")
                .expect("money resolved")
                .clone();
            let money2 = resolved
                .resolved
                .get("money2")
                .expect("money2 resolved")
                .clone();
            assert!(
                resolved.unit_index.unique_owner("eur").is_some(),
                "eur must be in unit_index"
            );
            assert!(
                resolved.unit_index.unique_owner("usd").is_some(),
                "usd must be in unit_index"
            );
            let eur_owner = resolved.unit_index.unique_owner("eur").expect("eur owner");
            let usd_owner = resolved.unit_index.unique_owner("usd").expect("usd owner");
            assert_eq!(
                eur_owner.name.as_deref(),
                Some("money"),
                "declarer money must own eur, not extension money2"
            );
            assert_eq!(
                usd_owner.name.as_deref(),
                Some("money2"),
                "money2 must own new unit usd"
            );
            assert_eq!(eur_owner.as_ref(), money.as_ref());
            assert_eq!(usd_owner.as_ref(), money2.as_ref());
        }

        #[test]
        fn unit_index_maps_family_root_not_binding_alias() {
            let code = r#"spec units
    data money: measure
      -> unit eur: 1
    data mass: measure
      -> unit kg: 1
    data price_per_weight: measure
      -> unit eur_per_kg: eur/kg

    spec consumer
    uses u: units
    data starch_levy_per_kg: u.price_per_weight
      -> suggest 0.1 eur_per_kg
    data amortization_per_kg: u.price_per_weight
      -> suggest 0.2 eur_per_kg
    "#;
            let (resolver, spec_arcs) = resolver_for_code(code);
            let units_arc = spec_arcs
                .iter()
                .find(|spec| spec.name == "units")
                .cloned()
                .expect("units spec");
            let consumer_arc = spec_arcs
                .iter()
                .find(|spec| spec.name == "consumer")
                .cloned()
                .expect("consumer spec");
            let (context, _) = test_context_and_effective(&spec_arcs);
            let repository = context.workspace();

            let mut already_resolved = ResolvedTypesMap::new();
            let units_types = resolver
                .resolve_and_validate(units_arc, &EffectiveDate::Origin, &already_resolved)
                .expect("units spec resolves");
            let units_price_per_weight = units_types
                .resolved
                .get("price_per_weight")
                .expect("units defines price_per_weight")
                .clone();
            already_resolved.push((Arc::clone(&repository), units_arc, units_types));

            let consumer_types = resolver
                .resolve_and_validate(consumer_arc, &EffectiveDate::Origin, &already_resolved)
                .expect("consumer spec resolves");

            let eur_per_kg_owner = consumer_types
                .unit_index
                .unique_owner("eur_per_kg")
                .expect("eur_per_kg in unit_index");
            assert_eq!(
                eur_per_kg_owner.name.as_deref(),
                Some("price_per_weight"),
                "unit must belong to family root, not binding alias row name"
            );
            assert_eq!(
                eur_per_kg_owner.measure_family_name(),
                Some("price_per_weight")
            );
            assert_eq!(eur_per_kg_owner.as_ref(), units_price_per_weight.as_ref());

            let starch_alias = consumer_types
                .resolved
                .get("starch_levy_per_kg")
                .expect("alias row in resolved");
            assert!(
                !std::ptr::eq(eur_per_kg_owner.as_ref(), starch_alias.as_ref()),
                "unit_index must not store binding alias arc"
            );
        }

        #[test]
        fn same_named_local_root_does_not_hide_imported_units() {
            let code = r#"spec std_units
    data mass: measure
      -> unit kilogram: 1
      -> unit kilograms: 1
      -> unit gram: 0.001

    spec s
    uses u: std_units
    data mass: measure
      -> unit kg: 1
      -> unit tonne: 1000
    rule smoke: true
    "#;
            let (resolver, spec_arcs) = resolver_for_code(code);
            let std_units_arc = spec_arcs
                .iter()
                .find(|spec| spec.name == "std_units")
                .cloned()
                .expect("std_units spec");
            let consumer_arc = spec_arcs
                .iter()
                .find(|spec| spec.name == "s")
                .cloned()
                .expect("consumer spec");
            let (context, _) = test_context_and_effective(&spec_arcs);
            let repository = context.workspace();

            let mut already_resolved = ResolvedTypesMap::new();
            let std_units_types = resolver
                .resolve_and_validate(std_units_arc, &EffectiveDate::Origin, &already_resolved)
                .expect("std_units resolves");
            already_resolved.push((Arc::clone(&repository), std_units_arc, std_units_types));

            let consumer_types = resolver
                .resolve_and_validate(consumer_arc, &EffectiveDate::Origin, &already_resolved)
                .expect("consumer resolves");

            let local_mass = consumer_types
                .resolved
                .get("mass")
                .expect("consumer defines mass")
                .clone();

            assert!(
                consumer_types.unit_index.unique_owner("kg").is_some(),
                "local kg must be indexed"
            );
            assert!(
                consumer_types.unit_index.unique_owner("tonne").is_some(),
                "local tonne must be indexed"
            );
            for imported_unit in ["kilogram", "kilograms", "gram"] {
                let owner = consumer_types
                    .unit_index
                    .unique_owner(imported_unit)
                    .unwrap_or_else(|| panic!("imported {imported_unit} must stay in scope"));
                assert_eq!(
                    owner.measure_family().map(|family| family.spec.as_str()),
                    Some("std_units"),
                    "{imported_unit} must belong to std_units.mass, not the local mass"
                );
            }

            let kg_owner = consumer_types
                .unit_index
                .unique_owner("kg")
                .expect("kg owner");
            let tonne_owner = consumer_types
                .unit_index
                .unique_owner("tonne")
                .expect("tonne owner");
            assert_eq!(kg_owner.as_ref(), local_mass.as_ref());
            assert_eq!(tonne_owner.as_ref(), local_mass.as_ref());
        }

        #[test]
        fn same_family_same_unit_same_factor_is_idempotent() {
            let code = r#"spec units_a
    data duration: measure
      -> unit hour: 1

    spec consumer
    uses a: units_a
    uses b: units_a
    rule smoke: true
    "#;
            let (resolver, spec_arcs) = resolver_for_code(code);
            let units_a_arc = spec_arcs
                .iter()
                .find(|spec| spec.name == "units_a")
                .cloned()
                .expect("units_a spec");
            let consumer_arc = spec_arcs
                .iter()
                .find(|spec| spec.name == "consumer")
                .cloned()
                .expect("consumer spec");
            let (context, _) = test_context_and_effective(&spec_arcs);
            let repository = context.workspace();

            let mut already_resolved = ResolvedTypesMap::new();
            let units_a_types = resolver
                .resolve_and_validate(units_a_arc, &EffectiveDate::Origin, &already_resolved)
                .expect("units_a resolves");
            already_resolved.push((Arc::clone(&repository), units_a_arc, units_a_types));

            let consumer_types = resolver
                .resolve_and_validate(consumer_arc, &EffectiveDate::Origin, &already_resolved)
                .expect("double import of same duration must be idempotent");

            assert_eq!(
                consumer_types.unit_index.owners_for("hour").len(),
                2,
                "each import path keeps its hour owner"
            );
            consumer_types
                .unit_index
                .resolve("hour")
                .expect("bare hour collapses across identical import paths");
        }

        #[test]
        fn different_version_families_keep_separate_hour_owners() {
            // Two versions are different families; merge keeps both owners.
            // Bare `hour` is ambiguous at use (see type_identity_versions).
            let code = r#"spec units 2024-01-01
    data duration: measure
      -> unit hour: 1

    spec units 2025-01-01
    data duration: measure
      -> unit hour: 60

    spec consumer
    uses a: units 2024-01-01
    uses b: units 2025-01-01
    rule smoke: true
    "#;
            let (resolver, spec_arcs) = resolver_for_code(code);
            let consumer_arc = spec_arcs
                .iter()
                .find(|spec| spec.name == "consumer")
                .cloned()
                .expect("consumer spec");
            let (context, _) = test_context_and_effective(&spec_arcs);
            let repository = context.workspace();

            let mut already_resolved = ResolvedTypesMap::new();
            for units_arc in spec_arcs.iter().filter(|spec| spec.name == "units") {
                let units_types = resolver
                    .resolve_and_validate(units_arc, &units_arc.effective_from, &already_resolved)
                    .expect("units version resolves");
                already_resolved.push((Arc::clone(&repository), units_arc, units_types));
            }

            let consumer_types = resolver
                .resolve_and_validate(consumer_arc, &EffectiveDate::Origin, &already_resolved)
                .expect("different version families resolve without merge conflict");
            assert_eq!(
                consumer_types.unit_index.owners_for("hour").len(),
                2,
                "each version keeps its own hour owner"
            );
        }

        #[test]
        fn test_spec_level_same_unit_names_keep_every_declarer() {
            let (resolver, spec) = resolver_single_spec(
                r#"spec test
    data money_a: measure
      -> unit eur: 1.00
      -> unit usd: 0.84

    data money_b: measure
      -> unit eur: 1.00
      -> unit usd: 1.20

    data length_a: measure
      -> unit meter: 1.0

    data length_b: measure
      -> unit meter: 1.0"#,
            );

            let resolved = resolver
                .resolve_and_validate(spec, &EffectiveDate::Origin, &Vec::new())
                .expect("independent declarers of one unit name must load");
            assert_eq!(
                resolved.unit_index.owners_for("eur").len(),
                2,
                "every declarer of 'eur' stays in scope"
            );
            assert!(
                resolved.unit_index.resolve("eur").is_err(),
                "bare 'eur' with two declarers must be qualified"
            );
            assert_eq!(
                resolved.unit_index.owners_for("usd").len(),
                2,
                "every declarer of 'usd' stays in scope"
            );
            assert!(
                resolved.unit_index.resolve("usd").is_err(),
                "bare 'usd' with two declarers must be qualified"
            );
            assert_eq!(
                resolved.unit_index.owners_for("meter").len(),
                2,
                "every declarer of 'meter' stays in scope"
            );
            assert!(
                resolved.unit_index.resolve("meter").is_err(),
                "bare 'meter' with two declarers must be qualified"
            );
            let (_, owner) = resolved
                .unit_index
                .resolve("money_b.usd")
                .expect("qualified unit must resolve");
            assert_eq!(owner.name(), "money_b");
        }

        #[test]
        fn test_measure_and_ratio_sharing_a_unit_name_keep_both_declarers() {
            let (resolver, spec) = resolver_single_spec(
                r#"spec test
    data q: measure
      -> unit foo: 1

    data r: ratio
      -> unit foo: 100"#,
            );

            let resolved = resolver
                .resolve_and_validate(spec, &EffectiveDate::Origin, &Vec::new())
                .expect("independent declarers of one unit name must load");
            assert_eq!(
                resolved.unit_index.owners_for("foo").len(),
                2,
                "every declarer of 'foo' stays in scope"
            );
            assert!(
                resolved.unit_index.resolve("foo").is_err(),
                "bare 'foo' with two declarers must be qualified"
            );
            let (_, owner) = resolved
                .unit_index
                .resolve("r.foo")
                .expect("qualified unit must resolve");
            assert_eq!(owner.name(), "r");
        }

        #[test]
        fn test_same_ratio_unit_across_types_keeps_both_declarers() {
            let (resolver, spec) = resolver_single_spec(
                r#"spec test
    data spread_a: ratio
      -> unit basis_points: 10000

    data spread_b: ratio
      -> unit basis_points: 10000"#,
            );

            let resolved = resolver
                .resolve_and_validate(spec, &EffectiveDate::Origin, &Vec::new())
                .expect("independent declarers of one unit name must load");
            assert_eq!(
                resolved.unit_index.owners_for("basis_points").len(),
                2,
                "every declarer of 'basis_points' stays in scope"
            );
            assert!(
                resolved.unit_index.resolve("basis_points").is_err(),
                "bare 'basis_points' with two declarers must be qualified"
            );
            let (_, owner) = resolved
                .unit_index
                .resolve("spread_a.basis_points")
                .expect("qualified unit must resolve");
            assert_eq!(owner.name(), "spread_a");
        }

        #[test]
        fn test_different_ratio_unit_factor_across_types_keeps_both_declarers() {
            let (resolver, spec) = resolver_single_spec(
                r#"spec test
    data spread_a: ratio
      -> unit basis_points: 10000

    data spread_b: ratio
      -> unit basis_points: 5000"#,
            );

            let resolved = resolver
                .resolve_and_validate(spec, &EffectiveDate::Origin, &Vec::new())
                .expect("independent declarers of one unit name must load");
            assert_eq!(
                resolved.unit_index.owners_for("basis_points").len(),
                2,
                "every declarer of 'basis_points' stays in scope"
            );
            assert!(
                resolved.unit_index.resolve("basis_points").is_err(),
                "bare 'basis_points' with two declarers must be qualified"
            );
            let (_, owner) = resolved
                .unit_index
                .resolve("spread_b.basis_points")
                .expect("qualified unit must resolve");
            assert_eq!(owner.name(), "spread_b");
        }

        #[test]
        fn test_multiple_builtin_ratio_types_share_percent_in_unit_index() {
            let (resolver, spec) = resolver_single_spec(
                r#"spec targets
    data standard_margin_pct: ratio
      -> minimum 0%
      -> suggest 15%

    data default_credit_insurance_pct: ratio
      -> suggest 1.5%"#,
            );

            resolver
                .resolve_and_validate(spec, &EffectiveDate::Origin, &Vec::new())
                .expect("multiple ratio types with built-in percent must not conflict");
        }

        #[test]
        fn test_three_ratio_types_share_builtin_and_custom_unit() {
            let (resolver, spec) = resolver_single_spec(
                r#"spec test
    data margin: ratio -> suggest 10%
    data fee: ratio
      -> unit tenths: 10
    data tax: ratio -> suggest 1%"#,
            );

            resolver
                .resolve_and_validate(spec, &EffectiveDate::Origin, &Vec::new())
                .expect("builtin percent/permille plus custom tenths on second type must load");
        }

        #[test]
        fn test_ratio_unit_index_allows_builtin_after_two_named_types() {
            let (resolver, spec) = resolver_single_spec(
                r#"spec test
    data first_ratio: ratio -> suggest 5%
    data second_ratio: ratio -> suggest 10%"#,
            );

            let resolved = resolver
                .resolve_and_validate(spec, &EffectiveDate::Origin, &Vec::new())
                .expect("two ratio types with only builtin units");

            let (_, percent) = resolved
                .unit_index
                .resolve("percent")
                .expect("bare percent binds PrimitiveRatio");
            assert_eq!(percent.name(), "ratio");
            let (_, permille) = resolved
                .unit_index
                .resolve("permille")
                .expect("bare permille binds PrimitiveRatio");
            assert_eq!(permille.name(), "ratio");
        }

        #[test]
        fn test_number_type_cannot_have_units() {
            let (resolver, spec) = resolver_single_spec(
                r#"spec test
    data price: number
      -> unit eur: 1.00"#,
            );

            let result = resolver.resolve_and_validate(spec, &EffectiveDate::Origin, &Vec::new());
            assert!(result.is_err(), "Number types must reject unit commands");

            let errs = result.unwrap_err();
            assert!(!errs.is_empty(), "expected at least one error");
            let error_msg = errs[0].to_string();
            assert!(
                error_msg.contains("unit") && error_msg.contains("number"),
                "Error should mention units are invalid on number. Got: {}",
                error_msg
            );
        }

        #[test]
        fn test_extending_type_inherits_units() {
            let (resolver, spec) = resolver_single_spec(
                r#"spec test
    data money: measure
      -> unit eur: 1.00
      -> unit usd: 0.84

    data my_money: money
      -> unit gbp: 1.30"#,
            );

            let resolved = resolver
                .resolve_and_validate(spec, &EffectiveDate::Origin, &Vec::new())
                .unwrap();
            let my_money_type = resolved.resolved.get("my_money").unwrap();

            match &my_money_type.specifications {
                TypeSpecification::Measure { units, .. } => {
                    assert_eq!(units.len(), 3);
                    assert!(units.iter().any(|u| u.name == "eur"));
                    assert!(units.iter().any(|u| u.name == "usd"));
                    assert!(units.iter().any(|u| u.name == "gbp"));
                }
                other => panic!("Expected Measure type specifications, got {:?}", other),
            }
        }

        #[test]
        fn binding_unit_factor_override_errors() {
            let (resolver, spec) = resolver_single_spec(
                r#"spec test
    data source_measure: measure
      -> unit usd: 1.00
    data z: source_measure
      -> unit usd: 0.84"#,
            );

            let result = resolver.resolve_and_validate(spec, &EffectiveDate::Origin, &Vec::new());
            assert!(
                result.is_err(),
                "binding row must not override inherited unit factor"
            );
            let error_msg = result
                .unwrap_err()
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join("; ");
            assert!(
                error_msg.contains("usd"),
                "error must name unit usd, got: {error_msg}"
            );
        }

        #[test]
        fn ratio_extension_unit_factor_override_errors() {
            let (resolver, spec) = resolver_single_spec(
                r#"spec test
    data parent: ratio
      -> unit basis: 1
    data child: parent
      -> unit basis: 100"#,
            );

            let result = resolver.resolve_and_validate(spec, &EffectiveDate::Origin, &Vec::new());
            assert!(
                result.is_err(),
                "ratio extension must not override inherited unit factor"
            );
            let error_msg = result
                .unwrap_err()
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join("; ");
            assert!(
                error_msg.contains("basis"),
                "error must name unit basis, got: {error_msg}"
            );
        }

        #[test]
        fn test_duplicate_unit_name_in_same_type_is_error() {
            let (resolver, spec) = resolver_single_spec(
                r#"spec test
    data money: measure
      -> unit eur: 1.00
      -> unit eur: 1.19"#,
            );

            let result = resolver.resolve_and_validate(spec, &EffectiveDate::Origin, &Vec::new());
            assert!(result.is_err(), "duplicate unit name must be rejected");
            let errs = result.unwrap_err();
            assert!(!errs.is_empty());
            let msg = errs[0].to_string();
            assert!(
                msg.contains("eur"),
                "error must name the duplicate unit; got: {msg}"
            );
        }

        #[test]
        fn test_duplicate_unit_name_compound_is_error() {
            let (resolver, spec) = resolver_single_spec(
                r#"spec test
    data currency: measure
      -> unit usd: 1.00
      -> unit eur: 0.92

    data time_period: measure
      -> unit year: 1

    data run_rate: measure
      -> unit arr: usd/year
      -> unit arr: eur/year"#,
            );

            let result = resolver.resolve_and_validate(spec, &EffectiveDate::Origin, &Vec::new());
            assert!(
                result.is_err(),
                "duplicate compound unit name must be rejected"
            );
            let errs = result.unwrap_err();
            assert!(!errs.is_empty());
            let msg = errs[0].to_string();
            assert!(
                msg.contains("arr"),
                "error must name the duplicate unit; got: {msg}"
            );
        }

        #[test]
        fn find_unique_reports_multiple_families_with_same_decomposition() {
            let code = r#"spec units
    data money: measure
      -> unit eur: 1
      -> unit usd: 0.86
    data mass: measure
      -> unit kg: 1
    data price_eur_per_kg: measure
      -> unit eur_per_kg: eur/kg
    data price_usd_per_kg: measure
      -> unit usd_per_kg: usd/kg
    "#;
            let (resolver, spec_arcs) = resolver_for_code(code);
            let units_arc = spec_arcs
                .iter()
                .find(|spec| spec.name == "units")
                .cloned()
                .expect("units spec");
            let (context, _) = test_context_and_effective(&spec_arcs);
            let repository = context.workspace();
            let resolved = resolver
                .resolve_and_validate(units_arc, &EffectiveDate::Origin, &ResolvedTypesMap::new())
                .expect("units resolves");
            let eur_decomp = resolved
                .unit_index
                .unique_owner("eur_per_kg")
                .expect("eur_per_kg")
                .measure_type_decomposition()
                .expect("eur_per_kg decomposition")
                .clone();
            let map = [(repository, units_arc, resolved)];

            let units_types = map
                .iter()
                .find(|(_, s, _)| discovery::same_loaded_spec(s, units_arc))
                .map(|(_, _, t)| t)
                .expect("units in map");
            let unique =
                find_unique_measure_type_in_unit_index(&units_types.unit_index, &eur_decomp);
            match unique {
                DecompositionMatch::Multiple(families) => {
                    assert_eq!(families.len(), 2);
                    assert!(families.contains(&"price_eur_per_kg".to_string()));
                    assert!(families.contains(&"price_usd_per_kg".to_string()));
                }
                other => panic!("expected Multiple families, got {other:?}"),
            }
        }
    }

    mod type_validation {
        use super::*;
        use crate::computation::rational::rational_new;
        use crate::literals::UnitScale;
        use crate::parsing::ast::{CommandArg, TypeConstraintCommand};
        use crate::planning::semantics::TypeSpecification;
        use rust_decimal::Decimal;

        fn test_source() -> Source {
            Source::new(
                crate::parsing::source::SourceType::Volatile,
                crate::parsing::ast::Span {
                    start: 0,
                    end: 0,
                    line: 1,
                    col: 0,
                },
            )
        }

        fn apply(
            mut specs: TypeSpecification,
            command: TypeConstraintCommand,
            args: &[CommandArg],
        ) -> TypeSpecification {
            let mut suggestion: Option<RawSuggestion> = None;
            specs
                .apply_constraint(
                    ConstraintNames {
                        type_name: "test",
                        data_name: "test",
                    },
                    command,
                    args,
                    &mut suggestion,
                    &mut None,
                    None,
                )
                .unwrap();
            specs
        }

        fn number_arg(n: i64) -> CommandArg {
            CommandArg::Literal(crate::literals::Value::Number(Decimal::from(n)))
        }

        fn date_arg(s: &str) -> CommandArg {
            let dt = s.parse::<crate::literals::DateTimeValue>().expect("date");
            CommandArg::Literal(crate::literals::Value::Date(dt))
        }

        fn time_arg(s: &str) -> CommandArg {
            let t = s.parse::<crate::literals::TimeValue>().expect("time");
            CommandArg::Literal(crate::literals::Value::Time(t))
        }

        #[test]
        fn validate_number_minimum_greater_than_maximum() {
            let mut specs = TypeSpecification::number();
            specs = apply(specs, TypeConstraintCommand::Minimum, &[number_arg(100)]);
            specs = apply(specs, TypeConstraintCommand::Maximum, &[number_arg(50)]);

            let src = test_source();
            let errors = validate_type_specifications(
                &specs,
                None,
                "suggestion",
                "test",
                &src,
                None,
                &UnitIndex::new(),
            );
            assert_eq!(errors.len(), 1);
            assert!(errors[0]
                .to_string()
                .contains("minimum 100 is greater than maximum 50"));
        }

        #[test]
        fn validate_number_default_below_minimum() {
            let specs = TypeSpecification::Number {
                minimum: Some(rational_new(10, 1)),
                maximum: None,
                decimals: None,
                help: String::new(),
            };
            let default = BoundValueKind::unbound(ValueKind::Number(rational_new(5, 1)));

            let src = test_source();
            let errors = validate_type_specifications(
                &specs,
                Some(&default),
                "suggestion",
                "test",
                &src,
                None,
                &UnitIndex::new(),
            );
            assert_eq!(errors.len(), 1);
            assert!(errors[0]
                .to_string()
                .contains("suggestion value 5 is less than minimum 10"));
        }

        #[test]
        fn validate_number_default_above_maximum() {
            let specs = TypeSpecification::Number {
                minimum: None,
                maximum: Some(rational_new(100, 1)),
                decimals: None,
                help: String::new(),
            };
            let default = BoundValueKind::unbound(ValueKind::Number(rational_new(150, 1)));

            let src = test_source();
            let errors = validate_type_specifications(
                &specs,
                Some(&default),
                "suggestion",
                "test",
                &src,
                None,
                &UnitIndex::new(),
            );
            assert_eq!(errors.len(), 1);
            assert!(errors[0]
                .to_string()
                .contains("suggestion value 150 is greater than maximum 100"));
        }

        #[test]
        fn validate_number_default_valid() {
            let specs = TypeSpecification::Number {
                minimum: Some(rational_new(0, 1)),
                maximum: Some(rational_new(100, 1)),
                decimals: None,
                help: String::new(),
            };
            let default = BoundValueKind::unbound(ValueKind::Number(rational_new(50, 1)));

            let src = test_source();
            let errors = validate_type_specifications(
                &specs,
                Some(&default),
                "suggestion",
                "test",
                &src,
                None,
                &UnitIndex::new(),
            );
            assert!(errors.is_empty());
        }

        #[test]
        fn validate_ratio_minimum_greater_than_maximum() {
            let specs = TypeSpecification::Ratio {
                minimum: Some(rational_new(2, 1)),
                maximum: Some(rational_new(1, 1)),
                units: crate::planning::semantics::RatioUnits::new(),
                unit: None,
                help: String::new(),
            };

            let src = test_source();
            let errors = validate_type_specifications(
                &specs,
                None,
                "suggestion",
                "test",
                &src,
                None,
                &UnitIndex::new(),
            );
            assert_eq!(errors.len(), 1);
            assert!(errors[0]
                .to_string()
                .contains("minimum 2 is greater than maximum 1"));
        }

        #[test]
        fn validate_date_minimum_after_maximum() {
            let mut specs = TypeSpecification::date();
            specs = apply(
                specs,
                TypeConstraintCommand::Minimum,
                &[date_arg("2024-12-31")],
            );
            specs = apply(
                specs,
                TypeConstraintCommand::Maximum,
                &[date_arg("2024-01-01")],
            );

            let src = test_source();
            let errors = validate_type_specifications(
                &specs,
                None,
                "suggestion",
                "test",
                &src,
                None,
                &UnitIndex::new(),
            );
            assert_eq!(errors.len(), 1);
            assert!(
                errors[0].to_string().contains("minimum")
                    && errors[0].to_string().contains("is after maximum")
            );
        }

        #[test]
        fn validate_date_valid_range() {
            let mut specs = TypeSpecification::date();
            specs = apply(
                specs,
                TypeConstraintCommand::Minimum,
                &[date_arg("2024-01-01")],
            );
            specs = apply(
                specs,
                TypeConstraintCommand::Maximum,
                &[date_arg("2024-12-31")],
            );

            let src = test_source();
            let errors = validate_type_specifications(
                &specs,
                None,
                "suggestion",
                "test",
                &src,
                None,
                &UnitIndex::new(),
            );
            assert!(errors.is_empty());
        }

        #[test]
        fn validate_time_minimum_after_maximum() {
            let mut specs = TypeSpecification::time();
            specs = apply(
                specs,
                TypeConstraintCommand::Minimum,
                &[time_arg("23:00:00")],
            );
            specs = apply(
                specs,
                TypeConstraintCommand::Maximum,
                &[time_arg("10:00:00")],
            );

            let src = test_source();
            let errors = validate_type_specifications(
                &specs,
                None,
                "suggestion",
                "test",
                &src,
                None,
                &UnitIndex::new(),
            );
            assert_eq!(errors.len(), 1);
            assert!(
                errors[0].to_string().contains("minimum")
                    && errors[0].to_string().contains("is after maximum")
            );
        }

        fn magnitude_beyond_decimal_max() -> crate::computation::rational::RationalInteger {
            use crate::computation::rational::{decimal_to_rational, rational_new, try_mul};
            use rust_decimal::Decimal;
            let max = Decimal::MAX.normalize();
            let max_rational = decimal_to_rational(max).expect("BUG: Decimal::MAX must lift to Q");
            try_mul(&max_rational, &rational_new(2, 1))
                .expect("BUG: test rational multiply must succeed")
        }

        #[test]
        fn large_magnitude_minimum_does_not_fail_type_validation() {
            use crate::computation::rational::rational_one;
            use crate::literals::MeasureUnits;
            use crate::planning::semantics::TypeSpecification;

            let too_large = magnitude_beyond_decimal_max();
            assert_eq!(
                too_large.try_to_decimal().unwrap_err(),
                crate::computation::rational::NumericFailure::Overflow,
            );

            let spec = TypeSpecification::Measure {
                minimum: Some((too_large, "eur".to_string())),
                maximum: None,
                units: MeasureUnits(vec![crate::literals::MeasureUnit {
                    name: "eur".to_string(),
                    scale: UnitScale::Literal(rational_one()),
                    derived_measure_factors: Default::default(),
                    decomposition: Default::default(),
                    minimum: None,
                    maximum: None,
                    suggestion_magnitude: None,
                }]),
                traits: vec![],
                decomposition: None,
                unit: None,
                help: String::new(),
            };

            let src = test_source();
            let errors = validate_type_specifications(
                &spec,
                None,
                "suggestion",
                "money",
                &src,
                None,
                &UnitIndex::new(),
            );
            assert!(
                errors.is_empty(),
                "internal Q bounds must not fail type validation; API decimal rounding: {:?}",
                errors.iter().map(|e| e.to_string()).collect::<Vec<_>>()
            );
        }

        #[test]
        fn large_magnitude_maximum_does_not_fail_type_validation() {
            use crate::computation::rational::rational_one;
            use crate::literals::MeasureUnits;
            use crate::planning::semantics::TypeSpecification;

            let too_large = magnitude_beyond_decimal_max();
            assert_eq!(
                too_large.try_to_decimal().unwrap_err(),
                crate::computation::rational::NumericFailure::Overflow,
            );

            let spec = TypeSpecification::Measure {
                minimum: None,
                maximum: Some((too_large, "eur".to_string())),
                units: MeasureUnits(vec![crate::literals::MeasureUnit {
                    name: "eur".to_string(),
                    scale: UnitScale::Literal(rational_one()),
                    derived_measure_factors: Default::default(),
                    decomposition: Default::default(),
                    minimum: None,
                    maximum: None,
                    suggestion_magnitude: None,
                }]),
                traits: vec![],
                decomposition: None,
                unit: None,
                help: String::new(),
            };

            let src = test_source();
            let errors = validate_type_specifications(
                &spec,
                None,
                "suggestion",
                "money",
                &src,
                None,
                &UnitIndex::new(),
            );
            assert!(errors.is_empty(), "got: {:?}", errors);
        }

        #[test]
        fn large_magnitude_default_does_not_fail_type_validation() {
            use crate::computation::rational::rational_one;
            use crate::literals::MeasureUnits;
            use crate::planning::semantics::TypeSpecification;

            let too_large = magnitude_beyond_decimal_max();
            assert_eq!(
                too_large.try_to_decimal().unwrap_err(),
                crate::computation::rational::NumericFailure::Overflow,
            );

            let spec = TypeSpecification::Measure {
                minimum: None,
                maximum: None,
                units: MeasureUnits(vec![crate::literals::MeasureUnit {
                    name: "eur".to_string(),
                    scale: UnitScale::Literal(rational_one()),
                    derived_measure_factors: Default::default(),
                    decomposition: Default::default(),
                    minimum: None,
                    maximum: None,
                    suggestion_magnitude: Some(too_large),
                }]),
                traits: vec![],
                decomposition: None,
                unit: None,
                help: String::new(),
            };

            let src = test_source();
            let errors = validate_type_specifications(
                &spec,
                None,
                "suggestion",
                "money",
                &src,
                None,
                &UnitIndex::new(),
            );
            assert!(errors.is_empty(), "got: {:?}", errors);
        }

        #[test]
        fn refresh_named_range_specs_missing_element_must_not_leave_unconverted_measure() {
            use crate::engine::Context;
            use crate::parsing::ast::{LemmaSpec, ParentType, Span};
            use crate::parsing::source::{Source, SourceType};
            use crate::planning::semantics::tests::primitive_measure_arc;

            let mut ctx = Context::new();
            let repository = ctx.workspace();
            ctx.insert_spec(Arc::clone(&repository), LemmaSpec::new("t".to_string()))
                .unwrap();
            let ctx: &'static Context = Box::leak(Box::new(ctx));
            let mut resolver = TypeResolver::new(ctx, HashMap::new());
            let spec = ctx
                .spec_set(&ctx.workspace(), "t")
                .and_then(|ss| ss.get_exact(None))
                .expect("inserted");
            assert!(resolver.register_all(&ctx.workspace(), spec).is_empty());
            let source = Source::new(
                SourceType::Volatile,
                Span {
                    start: 0,
                    end: 0,
                    line: 1,
                    col: 0,
                },
            );
            let mut data_defs = HashMap::new();
            data_defs.insert(
                "band".to_string(),
                DataTypeDef {
                    parent: ParentType::Ranged {
                        inner: Box::new(ParentType::custom("ghost")),
                    },
                    constraints: None,
                    source: source.clone(),
                    name: "band".to_string(),
                    bound_literal: None,
                },
            );
            let mut resolved_types = ResolvedSpecTypes::default();
            resolved_types
                .resolved
                .insert("band".to_string(), primitive_measure_arc().clone());
            let already_resolved = ResolvedTypesMap::new();

            let errors = refresh_named_range_specs(
                &resolver,
                spec,
                &data_defs,
                &mut resolved_types,
                &already_resolved,
                &EffectiveDate::Origin,
            );

            assert!(
                !errors.is_empty(),
                "planning must error when ranged inner type is missing"
            );
            assert!(
                errors[0]
                    .to_string()
                    .contains("Unknown parent 'ghost' for data definition"),
                "got: {}",
                errors[0]
            );
        }
    }
}
