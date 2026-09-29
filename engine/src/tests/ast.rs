use crate::computation::rational::rational_new;
use crate::literals::DateGranularity;
use crate::parsing::ast::*;
use crate::planning::semantics::{
    date_time_to_semantic, time_to_semantic, BaseMeasureVector, LemmaType, MeasureTrait,
    MeasureUnit, MeasureUnits, TypeExtends, TypeSpecification, TypedLiteral,
};
use rust_decimal::Decimal;

#[test]
fn test_literal_value_to_primitive_type() {
    let one = rational_new(1, 1);

    assert_eq!(TypedLiteral::text("".to_string()).lemma_type.name(), "text");
    assert_eq!(
        TypedLiteral::number(one.clone()).lemma_type.name(),
        "number"
    );
    assert_eq!(
        TypedLiteral::from_bool(bool::from(BooleanValue::True))
            .lemma_type
            .name(),
        "boolean"
    );

    let dt = DateTimeValue {
        year: 2024,
        month: 1,
        day: 1,
        hour: 0,
        minute: 0,
        second: 0,
        microsecond: 0,
        timezone: None,

        granularity: DateGranularity::Full,
    };
    assert_eq!(
        TypedLiteral::date(date_time_to_semantic(&dt))
            .lemma_type
            .name(),
        "date"
    );
    assert_eq!(
        TypedLiteral::ratio(rational_new(1, 2)).lemma_type.name(),
        "ratio"
    );
    let dur_type = LemmaType::new(
        "duration".to_string(),
        TypeSpecification::Measure {
            minimum: None,
            maximum: None,
            decimals: None,
            units: MeasureUnits::from(vec![MeasureUnit {
                name: "second".to_string(),
                factor: crate::computation::rational::rational_one(),
                derived_measure_factors: Vec::new(),
                decomposition: BaseMeasureVector::new(),
                minimum: None,
                maximum: None,
                suggestion_magnitude: None,
            }]),
            traits: vec![MeasureTrait::Duration],
            decomposition: None,
            unit: None,
            help: String::new(),
        },
        TypeExtends::Primitive,
    );
    assert_eq!(
        TypedLiteral::measure_with_type(one.clone(), std::sync::Arc::new(dur_type))
            .lemma_type
            .name(),
        "duration"
    );
}

#[test]
fn test_arithmetic_operation_display() {
    assert_eq!(format!("{}", ArithmeticComputation::Add), "+");
    assert_eq!(format!("{}", ArithmeticComputation::Subtract), "-");
    assert_eq!(format!("{}", ArithmeticComputation::Multiply), "*");
    assert_eq!(format!("{}", ArithmeticComputation::Divide), "/");
    assert_eq!(format!("{}", ArithmeticComputation::Modulo), "%");
    assert_eq!(format!("{}", ArithmeticComputation::Power), "^");
}

#[test]
fn test_comparison_operator_display() {
    assert_eq!(format!("{}", ComparisonComputation::GreaterThan), ">");
    assert_eq!(format!("{}", ComparisonComputation::LessThan), "<");
    assert_eq!(
        format!("{}", ComparisonComputation::GreaterThanOrEqual),
        ">="
    );
    assert_eq!(format!("{}", ComparisonComputation::LessThanOrEqual), "<=");
    assert_eq!(format!("{}", ComparisonComputation::Is), "is");
    assert_eq!(format!("{}", ComparisonComputation::IsNot), "is not");
}

#[test]
fn test_conversion_target_display() {
    assert_eq!(
        format!("{}", ConversionTarget::Type(PrimitiveKind::Number)),
        "number"
    );
}

#[test]
fn test_spec_type_display() {
    assert_eq!(
        format!(
            "{}",
            crate::planning::semantics::primitive_text_arc().as_ref()
        ),
        "text"
    );
    assert_eq!(
        format!(
            "{}",
            crate::planning::semantics::primitive_number_arc().as_ref()
        ),
        "number"
    );
    assert_eq!(
        format!(
            "{}",
            crate::planning::semantics::primitive_date_arc().as_ref()
        ),
        "date"
    );
    assert_eq!(
        format!(
            "{}",
            crate::planning::semantics::primitive_boolean_arc().as_ref()
        ),
        "boolean"
    );
    assert_eq!(
        format!(
            "{}",
            crate::planning::semantics::primitive_ratio_arc().as_ref()
        ),
        "ratio"
    );
    assert_eq!(
        format!("{}", crate::planning::semantics::tests::primitive_measure()),
        "measure"
    );
    assert_eq!(
        format!(
            "{}",
            crate::planning::semantics::primitive_time_arc().as_ref()
        ),
        "time"
    );
}

#[test]
fn test_type_constructor() {
    let specs = TypeSpecification::number();
    let lemma_type = LemmaType::new("dice".to_string(), specs, TypeExtends::Primitive);
    assert_eq!(lemma_type.name(), "dice");
}

#[test]
fn test_type_display() {
    let specs = TypeSpecification::text();
    let lemma_type = LemmaType::new("name".to_string(), specs, TypeExtends::Primitive);
    assert_eq!(format!("{}", lemma_type), "name");
}

#[test]
fn test_type_equality() {
    let specs1 = TypeSpecification::number();
    let specs2 = TypeSpecification::number();
    let lemma_type1 = LemmaType::new("dice".to_string(), specs1, TypeExtends::Primitive);
    let lemma_type2 = LemmaType::new("dice".to_string(), specs2, TypeExtends::Primitive);
    assert_eq!(lemma_type1, lemma_type2);

    let specs3 = TypeSpecification::number();
    let lemma_type3 = LemmaType::new("other_dice".to_string(), specs3, TypeExtends::Primitive);
    assert_ne!(
        lemma_type1, lemma_type3,
        "Types with different names must not be equal"
    );
}

#[test]
fn test_type_serialization() {
    let specs = TypeSpecification::number();
    let lemma_type = LemmaType::new("dice".to_string(), specs, TypeExtends::Primitive);
    let api = crate::api::LemmaType::from(&lemma_type);
    let serialized = serde_json::to_string(&api).unwrap();
    let deserialized: crate::api::LemmaType = serde_json::from_str(&serialized).unwrap();
    assert_eq!(api, deserialized);
}

#[test]
fn test_literal_value_display_value() {
    let ten = rational_new(10, 1);
    let ten_hours_canonical = rational_new(36_000, 1);

    assert_eq!(
        TypedLiteral::text("hello".to_string()).display_value(),
        "hello"
    );
    assert_eq!(TypedLiteral::number(ten).display_value(), "10");
    assert_eq!(TypedLiteral::from_bool(true).display_value(), "true");
    assert_eq!(TypedLiteral::from_bool(false).display_value(), "false");

    let ten_percent_ratio = TypedLiteral::ratio(rational_new(1, 10));
    assert_eq!(ten_percent_ratio.display_value(), "10%");

    let date = DateTimeValue {
        year: 2024,
        month: 6,
        day: 15,
        hour: 0,
        minute: 0,
        second: 0,
        microsecond: 0,
        timezone: None,

        granularity: DateGranularity::Full,
    };
    assert_eq!(
        TypedLiteral::date(date_time_to_semantic(&date)).display_value(),
        "2024-06-15"
    );

    let datetime = DateTimeValue {
        year: 2024,
        month: 12,
        day: 25,
        hour: 14,
        minute: 30,
        second: 45,
        microsecond: 0,
        timezone: Some(TimezoneValue {
            offset_hours: 1,
            offset_minutes: 0,
        }),

        granularity: DateGranularity::DateTime,
    };
    assert_eq!(
        TypedLiteral::date(date_time_to_semantic(&datetime)).display_value(),
        "2024-12-25T14:30:45+01:00"
    );

    let time = TimeValue {
        hour: 14,
        minute: 30,
        second: 0,
        microsecond: 0,
        timezone: None,
    };
    assert_eq!(
        TypedLiteral::time(time_to_semantic(&time)).display_value(),
        "14:30:00"
    );

    let dur_type = LemmaType::new(
        "duration".to_string(),
        TypeSpecification::Measure {
            minimum: None,
            maximum: None,
            decimals: None,
            units: MeasureUnits::from(vec![MeasureUnit {
                name: "hour".to_string(),
                factor: crate::computation::rational::decimal_to_rational(Decimal::from(3600))
                    .expect("3600 must be exact decimal ratio"),
                derived_measure_factors: Vec::new(),
                decomposition: BaseMeasureVector::new(),
                minimum: None,
                maximum: None,
                suggestion_magnitude: None,
            }]),
            traits: vec![MeasureTrait::Duration],
            decomposition: None,
            unit: None,
            help: String::new(),
        },
        TypeExtends::Primitive,
    );
    assert_eq!(
        TypedLiteral::measure_with_type(ten_hours_canonical, std::sync::Arc::new(dur_type),)
            .display_value(),
        "10 hour"
    );
}

#[test]
fn test_literal_value_time_type() {
    let time = TimeValue {
        hour: 14,
        minute: 30,
        second: 0,
        microsecond: 0,
        timezone: None,
    };
    assert_eq!(
        TypedLiteral::time(time_to_semantic(&time))
            .lemma_type
            .name(),
        "time"
    );
}

#[test]
fn test_datetime_value_display() {
    let dt = DateTimeValue {
        year: 2024,
        month: 12,
        day: 25,
        hour: 14,
        minute: 30,
        second: 45,
        microsecond: 0,
        timezone: Some(TimezoneValue {
            offset_hours: 1,
            offset_minutes: 0,
        }),

        granularity: DateGranularity::DateTime,
    };
    let display = format!("{}", dt);
    assert_eq!(display, "2024-12-25T14:30:45+01:00");
}

#[test]
fn test_time_value_display() {
    let time = TimeValue {
        hour: 14,
        minute: 30,
        second: 45,
        microsecond: 0,
        timezone: Some(TimezoneValue {
            offset_hours: -5,
            offset_minutes: 30,
        }),
    };
    let display = format!("{}", time);
    assert_eq!(display, "14:30:45-05:30");
}

#[test]
fn test_timezone_value() {
    let tz_positive = TimezoneValue {
        offset_hours: 5,
        offset_minutes: 30,
    };
    assert_eq!(format!("{}", tz_positive), "+05:30");

    let tz_negative = TimezoneValue {
        offset_hours: -8,
        offset_minutes: 0,
    };
    assert_eq!(format!("{}", tz_negative), "-08:00");

    let tz_utc = TimezoneValue {
        offset_hours: 0,
        offset_minutes: 0,
    };
    assert_eq!(format!("{}", tz_utc), "Z");
}

#[test]
fn test_negation_types() {
    let json = serde_json::to_string(&NegationType::Not).expect("serialize NegationType");
    let decoded: NegationType = serde_json::from_str(&json).expect("deserialize NegationType");
    assert_eq!(decoded, NegationType::Not);
}

#[test]
fn test_veto_expression() {
    let veto_with_message = VetoExpression {
        message: Some("Must be over 18".to_string()),
    };
    assert_eq!(
        veto_with_message.message,
        Some("Must be over 18".to_string())
    );

    let veto_without_message = VetoExpression { message: None };
    assert!(veto_without_message.message.is_none());
}

#[test]
fn test_datetime_value_parse_year_and_year_month_equal() {
    let from_year: DateTimeValue = "2026".parse().expect("2026 should parse");
    let from_year_month: DateTimeValue = "2026-01".parse().expect("2026-01 should parse");
    assert_eq!(
        from_year, from_year_month,
        "2026 and 2026-01 should normalize to same value"
    );
    assert_eq!(from_year.year, 2026);
    assert_eq!(from_year.month, 1);
    assert_eq!(from_year.day, 1);
    assert_eq!(from_year.hour, 0);
    assert_eq!(from_year.minute, 0);
    assert_eq!(from_year.second, 0);
}

#[test]
fn test_datetime_value_granularity_display() {
    let year: DateTimeValue = "2026".parse().expect("2026 should parse");
    assert_eq!(year.to_string(), "2026");
    assert_eq!(year.granularity, DateGranularity::Year);

    let year_month: DateTimeValue = "2026-01".parse().expect("2026-01 should parse");
    assert_eq!(year_month.to_string(), "2026-01");
    assert_eq!(year_month.granularity, DateGranularity::YearMonth);

    let year_month_march: DateTimeValue = "2026-03".parse().expect("2026-03 should parse");
    assert_eq!(year_month_march.to_string(), "2026-03");
    assert_eq!(year_month_march.granularity, DateGranularity::YearMonth);

    let full: DateTimeValue = "2026-01-01".parse().expect("2026-01-01 should parse");
    assert_eq!(full.to_string(), "2026-01-01");
    assert_eq!(full.granularity, DateGranularity::Full);

    let week_34: DateTimeValue = "2026-W34".parse().expect("2026-W34 should parse");
    assert_eq!(week_34.to_string(), "2026-W34");
    assert_eq!(
        week_34.granularity,
        DateGranularity::IsoWeek {
            iso_year: 2026,
            week: 34
        }
    );

    let week_01: DateTimeValue = "2026-W01".parse().expect("2026-W01 should parse");
    assert_eq!(week_01.to_string(), "2026-W01");
    assert_eq!(
        week_01.granularity,
        DateGranularity::IsoWeek {
            iso_year: 2026,
            week: 1
        }
    );
    assert_eq!(week_01.year, 2025);

    assert_eq!(
        "2026".parse::<DateTimeValue>().unwrap(),
        "2026-01-01".parse::<DateTimeValue>().unwrap()
    );
}
