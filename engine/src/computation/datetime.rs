//! DateTime operations
//!
//! Handles arithmetic and comparisons with dates and datetimes.
//! Returns OperationResult with Veto for errors instead of Result.

use crate::computation::operation_result::{OperationResult, VetoType};
use crate::computation::rational::{rational_new, RationalInteger};
use crate::planning::semantics::{
    ArithmeticComputation, ComparisonComputation, LiteralValue, SemanticCalendarUnit,
    SemanticDateTime, SemanticTime, SemanticTimezone, ValueKind,
};
use chrono::{
    DateTime, Datelike, Duration as ChronoDuration, FixedOffset, NaiveDate, NaiveDateTime,
    NaiveTime, TimeZone, Timelike,
};

const SECONDS_PER_HOUR: i32 = 3600;
const SECONDS_PER_MINUTE: i32 = 60;
const MICROSECONDS_PER_SECOND: i64 = 1_000_000;

const EPOCH_YEAR: i32 = 1970;
const EPOCH_MONTH: u32 = 1;
const EPOCH_DAY: u32 = 1;

fn create_semantic_timezone_offset(
    timezone: &Option<SemanticTimezone>,
) -> Result<FixedOffset, String> {
    if let Some(tz) = timezone {
        let offset_seconds = (tz.offset_hours as i32 * SECONDS_PER_HOUR)
            + (tz.offset_minutes as i32 * SECONDS_PER_MINUTE);
        FixedOffset::east_opt(offset_seconds).ok_or_else(|| {
            format!(
                "Invalid timezone offset: {}:{}",
                tz.offset_hours, tz.offset_minutes
            )
        })
    } else {
        FixedOffset::east_opt(0).ok_or_else(|| "Failed to create UTC offset".to_string())
    }
}

/// Perform date/datetime arithmetic, returning OperationResult (Veto on error)
pub fn datetime_arithmetic(
    left: &LiteralValue,
    left_type: &std::sync::Arc<crate::planning::semantics::LemmaType>,
    op: &ArithmeticComputation,
    right: &LiteralValue,
    right_type: &std::sync::Arc<crate::planning::semantics::LemmaType>,
) -> OperationResult {
    match (&left.value, &right.value, op) {
        (ValueKind::Date(date), ValueKind::Measure(_), ArithmeticComputation::Add)
            if right_type.is_duration_like_measure() =>
        {
            let dt = match semantic_datetime_to_chrono(date) {
                Ok(d) => d,
                Err(msg) => return OperationResult::Veto(VetoType::computation(msg)),
            };
            let second = right.duration_canonical_seconds(right_type.as_ref());
            let duration = match seconds_to_chrono_duration(&second) {
                Ok(d) => d,
                Err(msg) => return OperationResult::Veto(VetoType::computation(msg)),
            };
            let new_dt = match dt.checked_add_signed(duration) {
                Some(d) => d,
                None => return OperationResult::Veto(VetoType::computation("Date overflow")),
            };
            OperationResult::from_literal(LiteralValue::date_with_type(
                chrono_to_semantic_datetime(new_dt),
                std::sync::Arc::clone(left_type),
            ))
        }

        (ValueKind::Date(date), ValueKind::Measure(_), ArithmeticComputation::Add)
            if right_type.is_calendar_like() =>
        {
            let dt = match semantic_datetime_to_chrono(date) {
                Ok(d) => d,
                Err(msg) => return OperationResult::Veto(VetoType::computation(msg)),
            };
            let month = right.calendar_canonical_months(right_type.as_ref());
            let new_dt = match apply_calendar_to_datetime(dt, &month, true) {
                Ok(d) => d,
                Err(msg) => return OperationResult::Veto(VetoType::computation(msg)),
            };

            OperationResult::from_literal(LiteralValue::date_with_type(
                chrono_to_semantic_datetime(new_dt),
                std::sync::Arc::clone(left_type),
            ))
        }

        (
            ValueKind::Date(date),
            ValueKind::Measure(_),
            ArithmeticComputation::Subtract,
        ) if right_type.is_duration_like_measure() => {
            let dt = match semantic_datetime_to_chrono(date) {
                Ok(d) => d,
                Err(msg) => return OperationResult::Veto(VetoType::computation(msg)),
            };
            let second = right.duration_canonical_seconds(right_type.as_ref());
            let duration = match seconds_to_chrono_duration(&second) {
                Ok(d) => d,
                Err(msg) => return OperationResult::Veto(VetoType::computation(msg)),
            };
            let new_dt = match dt.checked_sub_signed(duration) {
                Some(d) => d,
                None => return OperationResult::Veto(VetoType::computation("Date overflow")),
            };
            OperationResult::from_literal(LiteralValue::date_with_type(
                chrono_to_semantic_datetime(new_dt),
                std::sync::Arc::clone(left_type),
            ))
        }

        (
            ValueKind::Date(date),
            ValueKind::Measure(_),
            ArithmeticComputation::Subtract,
        ) if right_type.is_calendar_like() => {
            let dt = match semantic_datetime_to_chrono(date) {
                Ok(d) => d,
                Err(msg) => return OperationResult::Veto(VetoType::computation(msg)),
            };
            let month = right.calendar_canonical_months(right_type.as_ref());
            let new_dt = match apply_calendar_to_datetime(dt, &month, false) {
                Ok(d) => d,
                Err(msg) => return OperationResult::Veto(VetoType::computation(msg)),
            };

            OperationResult::from_literal(LiteralValue::date_with_type(
                chrono_to_semantic_datetime(new_dt),
                std::sync::Arc::clone(left_type),
            ))
        }

        (ValueKind::Measure(_), ValueKind::Date(date), ArithmeticComputation::Add)
            if left_type.is_duration_like_measure() =>
        {
            let dt = match semantic_datetime_to_chrono(date) {
                Ok(d) => d,
                Err(msg) => return OperationResult::Veto(VetoType::computation(msg)),
            };
            let second = left.duration_canonical_seconds(left_type.as_ref());
            let duration = match seconds_to_chrono_duration(&second) {
                Ok(d) => d,
                Err(msg) => return OperationResult::Veto(VetoType::computation(msg)),
            };
            let new_dt = match dt.checked_add_signed(duration) {
                Some(d) => d,
                None => return OperationResult::Veto(VetoType::computation("Date overflow")),
            };
            OperationResult::from_literal(LiteralValue::date_with_type(
                chrono_to_semantic_datetime(new_dt),
                std::sync::Arc::clone(right_type),
            ))
        }

        (ValueKind::Measure(_), ValueKind::Date(date), ArithmeticComputation::Add)
            if left_type.is_calendar_like() =>
        {
            let dt = match semantic_datetime_to_chrono(date) {
                Ok(d) => d,
                Err(msg) => return OperationResult::Veto(VetoType::computation(msg)),
            };
            let month = left.calendar_canonical_months(left_type.as_ref());
            let new_dt = match apply_calendar_to_datetime(dt, &month, true) {
                Ok(d) => d,
                Err(msg) => return OperationResult::Veto(VetoType::computation(msg)),
            };

            OperationResult::from_literal(LiteralValue::date_with_type(
                chrono_to_semantic_datetime(new_dt),
                std::sync::Arc::clone(right_type),
            ))
        }

        (ValueKind::Date(date), ValueKind::Time(time), ArithmeticComputation::Subtract) => {
            // Date - Time: Create a datetime from the date's date components and the time's time components
            // Then subtract to get the duration
            let date_dt = match semantic_datetime_to_chrono(date) {
                Ok(d) => d,
                Err(msg) => return OperationResult::Veto(VetoType::computation(msg)),
            };

            // Create a datetime using the date's date components and the time's time components
            let naive_date = match NaiveDate::from_ymd_opt(date.year, date.month, date.day) {
                Some(d) => d,
                None => {
                    return OperationResult::Veto(VetoType::computation(format!(
                        "Invalid date: {}-{}-{}",
                        date.year, date.month, date.day
                    )))
                }
            };
            let naive_time = match NaiveTime::from_hms_micro_opt(
                time.hour,
                time.minute,
                time.second,
                time.microsecond,
            ) {
                Some(t) => t,
                None => {
                    return OperationResult::Veto(VetoType::computation(format!(
                        "Invalid time: {}:{}:{}.{}",
                        time.hour, time.minute, time.second, time.microsecond
                    )))
                }
            };
            let naive_dt = NaiveDateTime::new(naive_date, naive_time);

            // Use the date's timezone, or UTC if not specified
            let offset = match create_semantic_timezone_offset(&date.timezone) {
                Ok(o) => o,
                Err(msg) => return OperationResult::Veto(VetoType::computation(msg)),
            };
            let time_dt = match offset.from_local_datetime(&naive_dt).single() {
                Some(dt) => dt,
                None => {
                    return OperationResult::Veto(VetoType::computation(
                        "Ambiguous or invalid datetime for timezone",
                    ))
                }
            };

            let duration = date_dt - time_dt;
            let second = match chrono_duration_to_rational_seconds(duration) {
                Ok(value) => value,
                Err(message) => return OperationResult::Veto(VetoType::computation(message)),
            };
            OperationResult::from_literal(LiteralValue::measure(second))
        }

        _ => unreachable!(
            "BUG: datetime arithmetic {:?} for unsupported operand types {} vs {}; planning should have rejected this",
            op,
            left_type.name(),
            right_type.name()
        ),
    }
}

pub(crate) fn semantic_datetime_to_chrono(
    date: &SemanticDateTime,
) -> Result<DateTime<FixedOffset>, String> {
    let naive_date = NaiveDate::from_ymd_opt(date.year, date.month, date.day)
        .ok_or_else(|| format!("Invalid date: {}-{}-{}", date.year, date.month, date.day))?;

    let naive_time =
        NaiveTime::from_hms_micro_opt(date.hour, date.minute, date.second, date.microsecond)
            .ok_or_else(|| {
                format!(
                    "Invalid time: {}:{}:{}.{}",
                    date.hour, date.minute, date.second, date.microsecond
                )
            })?;

    let naive_dt = NaiveDateTime::new(naive_date, naive_time);

    let offset = create_semantic_timezone_offset(&date.timezone)?;
    offset
        .from_local_datetime(&naive_dt)
        .single()
        .ok_or_else(|| "Ambiguous or invalid datetime for timezone".to_string())
}

fn chrono_to_semantic_datetime(dt: DateTime<FixedOffset>) -> SemanticDateTime {
    let offset_seconds = dt.offset().local_minus_utc();
    let offset_hours = (offset_seconds / SECONDS_PER_HOUR) as i8;
    let offset_minutes = ((offset_seconds.abs() % SECONDS_PER_HOUR) / SECONDS_PER_MINUTE) as u8;

    SemanticDateTime {
        year: dt.year(),
        month: dt.month(),
        day: dt.day(),
        hour: dt.hour(),
        minute: dt.minute(),
        second: dt.second(),
        microsecond: dt.nanosecond() / 1000 % 1_000_000,
        timezone: Some(SemanticTimezone {
            offset_hours,
            offset_minutes,
        }),
    }
}

fn seconds_to_chrono_duration(second: &RationalInteger) -> Result<ChronoDuration, String> {
    use crate::computation::rational::checked_mul;
    let micros_per_sec = rational_new(MICROSECONDS_PER_SECOND, 1);
    let microsecond = checked_mul(second, &micros_per_sec)
        .map_err(|e| format!("Duration conversion overflow: {e}"))?;
    if !microsecond.is_integer() {
        return Err("Duration conversion requires microsecond precision".to_string());
    }
    let us_i64 = microsecond
        .numer_to_i128()
        .and_then(|v| i64::try_from(v).ok())
        .ok_or_else(|| "Duration conversion failed".to_string())?;
    Ok(ChronoDuration::microseconds(us_i64))
}

pub(crate) fn chrono_duration_to_rational_seconds(
    duration: ChronoDuration,
) -> Result<RationalInteger, String> {
    let microsecond = duration
        .num_microseconds()
        .ok_or_else(|| "Duration conversion failed".to_string())?;
    Ok(rational_new(microsecond, MICROSECONDS_PER_SECOND))
}

fn apply_calendar_to_datetime(
    dt: DateTime<FixedOffset>,
    value: &RationalInteger,
    add: bool,
) -> Result<DateTime<FixedOffset>, String> {
    let months_rational = value.clone();

    if !months_rational.is_integer() {
        return Err(format!(
            "Cannot apply fractional calendar offset ({} month) to a date",
            months_rational
        ));
    }

    let months_i32 = months_rational
        .numer_to_i32()
        .ok_or_else(|| "Calendar offset too large".to_string())?;

    let signed_months = if add {
        months_i32
    } else {
        months_i32
            .checked_neg()
            .ok_or_else(|| "Calendar offset too large".to_string())?
    };

    let base_months = dt
        .year()
        .checked_mul(12)
        .and_then(|y| y.checked_add(dt.month() as i32 - 1))
        .ok_or_else(|| "Calendar offset too large".to_string())?;
    let total_months = base_months
        .checked_add(signed_months)
        .ok_or_else(|| "Calendar offset too large".to_string())?;
    let target_year = total_months.div_euclid(12);
    let target_month = (total_months.rem_euclid(12) + 1) as u32;

    let max_day = days_in_month(target_year, target_month);
    let target_day = dt.day().min(max_day);

    let naive_date =
        NaiveDate::from_ymd_opt(target_year, target_month, target_day).ok_or_else(|| {
            format!("Invalid date after calendar offset: {target_year}-{target_month}-{target_day}")
        })?;
    let naive_time = dt.time();
    let naive_dt = NaiveDateTime::new(naive_date, naive_time);

    let offset = *dt.offset();
    offset
        .from_local_datetime(&naive_dt)
        .single()
        .ok_or_else(|| "Ambiguous or invalid datetime after calendar offset".to_string())
}

fn days_in_month(year: i32, month: u32) -> u32 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 => {
            if year % 4 == 0 && (year % 100 != 0 || year % 400 == 0) {
                29
            } else {
                28
            }
        }
        _ => unreachable!("BUG: month must be 1-12, got {}", month),
    }
}

pub fn compute_date_calendar_difference(
    left: &SemanticDateTime,
    right: &SemanticDateTime,
    unit: &SemanticCalendarUnit,
    lemma_type: std::sync::Arc<crate::planning::semantics::LemmaType>,
) -> OperationResult {
    let left_datetime = match semantic_datetime_to_chrono(left) {
        Ok(value) => value,
        Err(message) => return OperationResult::Veto(VetoType::computation(message)),
    };
    let right_datetime = match semantic_datetime_to_chrono(right) {
        Ok(value) => value,
        Err(message) => return OperationResult::Veto(VetoType::computation(message)),
    };

    let signed_full_months = signed_full_months_between(&left_datetime, &right_datetime);
    let abs_months = signed_full_months.unsigned_abs();
    // Calendar quantities store canonical month counts; year/month signature selects display unit.
    let value = rational_new(i64::from(abs_months), 1);
    let _ = unit;

    OperationResult::from_literal(LiteralValue::calendar(value, unit.clone(), lemma_type))
}

fn signed_full_months_between(left: &DateTime<FixedOffset>, right: &DateTime<FixedOffset>) -> i32 {
    if left == right {
        return 0;
    }

    let (earlier, later, sign) = if right >= left {
        (left, right, 1)
    } else {
        (right, left, -1)
    };

    let mut full_months =
        (later.year() - earlier.year()) * 12 + (later.month() as i32 - earlier.month() as i32);

    let later_before_earlier_day_time = later.day() < earlier.day()
        || (later.day() == earlier.day()
            && (
                later.hour(),
                later.minute(),
                later.second(),
                later.nanosecond() / 1000,
            ) < (
                earlier.hour(),
                earlier.minute(),
                earlier.second(),
                earlier.nanosecond() / 1000,
            ));

    if later_before_earlier_day_time {
        full_months -= 1;
    }

    sign * full_months
}

pub fn evaluate_past_future_range(
    kind: &DateRelativeKind,
    duration_or_calendar: &LiteralValue,
    duration_type: &std::sync::Arc<crate::planning::semantics::LemmaType>,
    now: &SemanticDateTime,
) -> OperationResult {
    let now_value = LiteralValue::date(now.clone());
    let shift_operator = match kind {
        DateRelativeKind::InPast => ArithmeticComputation::Subtract,
        DateRelativeKind::InFuture => ArithmeticComputation::Add,
    };

    let date_type = crate::planning::semantics::primitive_date_arc();
    let shifted = datetime_arithmetic(
        &now_value,
        date_type,
        &shift_operator,
        duration_or_calendar,
        duration_type,
    );
    let shifted_value = match shifted {
        OperationResult::Value(value) => value.to_literal(),
        OperationResult::Veto(reason) => return OperationResult::Veto(reason),
    };

    let range_value = match kind {
        DateRelativeKind::InPast => LiteralValue::range(shifted_value, now_value),
        DateRelativeKind::InFuture => LiteralValue::range(now_value, shifted_value),
    };

    OperationResult::from_literal(range_value)
}

/// Perform date/datetime comparisons, returning OperationResult (Veto on error)
pub fn datetime_comparison(
    left: &LiteralValue,
    left_type: &std::sync::Arc<crate::planning::semantics::LemmaType>,
    op: &ComparisonComputation,
    right: &LiteralValue,
    right_type: &std::sync::Arc<crate::planning::semantics::LemmaType>,
) -> OperationResult {
    match (&left.value, &right.value) {
        (ValueKind::Date(l), ValueKind::Date(r)) => {
            let l_dt = match semantic_datetime_to_chrono(l) {
                Ok(d) => d,
                Err(msg) => return OperationResult::Veto(VetoType::computation(msg)),
            };
            let r_dt = match semantic_datetime_to_chrono(r) {
                Ok(d) => d,
                Err(msg) => return OperationResult::Veto(VetoType::computation(msg)),
            };

            let l_utc = l_dt.naive_utc();
            let r_utc = r_dt.naive_utc();

            let result = match op {
                ComparisonComputation::GreaterThan => l_utc > r_utc,
                ComparisonComputation::LessThan => l_utc < r_utc,
                ComparisonComputation::GreaterThanOrEqual => l_utc >= r_utc,
                ComparisonComputation::LessThanOrEqual => l_utc <= r_utc,
                ComparisonComputation::Is => l_utc == r_utc,
                ComparisonComputation::IsNot => l_utc != r_utc,
            };

            OperationResult::from_literal(LiteralValue::from_bool(result))
        }

        _ => unreachable!(
            "BUG: datetime_comparison with non-date operands ({} vs {}); planning should have rejected this",
            left_type.name(),
            right_type.name()
        ),
    }
}

/// Perform time comparisons, returning OperationResult (Veto on error)
pub fn time_comparison(
    left: &LiteralValue,
    left_type: &std::sync::Arc<crate::planning::semantics::LemmaType>,
    op: &ComparisonComputation,
    right: &LiteralValue,
    right_type: &std::sync::Arc<crate::planning::semantics::LemmaType>,
) -> OperationResult {
    match (&left.value, &right.value) {
        (ValueKind::Time(l), ValueKind::Time(r)) => {
            let l_dt = match semantic_time_to_chrono_datetime(l) {
                Ok(d) => d,
                Err(msg) => return OperationResult::Veto(VetoType::computation(msg)),
            };
            let r_dt = match semantic_time_to_chrono_datetime(r) {
                Ok(d) => d,
                Err(msg) => return OperationResult::Veto(VetoType::computation(msg)),
            };

            let l_utc = l_dt.naive_utc();
            let r_utc = r_dt.naive_utc();

            let result = match op {
                ComparisonComputation::GreaterThan => l_utc > r_utc,
                ComparisonComputation::LessThan => l_utc < r_utc,
                ComparisonComputation::GreaterThanOrEqual => l_utc >= r_utc,
                ComparisonComputation::LessThanOrEqual => l_utc <= r_utc,
                ComparisonComputation::Is => l_utc == r_utc,
                ComparisonComputation::IsNot => l_utc != r_utc,
            };

            OperationResult::from_literal(LiteralValue::from_bool(result))
        }
        _ => unreachable!(
            "BUG: time_comparison called with non-time operands ({} vs {}); this should be enforced by planning and dispatch",
            left_type.name(),
            right_type.name()
        ),
    }
}

/// Perform time arithmetic operations, returning OperationResult (Veto on error)
pub fn time_arithmetic(
    left: &LiteralValue,
    left_type: &std::sync::Arc<crate::planning::semantics::LemmaType>,
    op: &ArithmeticComputation,
    right: &LiteralValue,
    right_type: &std::sync::Arc<crate::planning::semantics::LemmaType>,
) -> OperationResult {
    match (&left.value, &right.value, op) {
        (ValueKind::Time(time), ValueKind::Measure(_), ArithmeticComputation::Add)
            if right_type.is_duration_like_measure() =>
        {
            let second = right.duration_canonical_seconds(right_type.as_ref());
            let time_aware = match semantic_time_to_chrono_datetime(time) {
                Ok(d) => d,
                Err(msg) => return OperationResult::Veto(VetoType::computation(msg)),
            };
            let duration = match seconds_to_chrono_duration(&second) {
                Ok(d) => d,
                Err(msg) => return OperationResult::Veto(VetoType::computation(msg)),
            };
            let result_dt = match time_aware.checked_add_signed(duration) {
                Some(d) => d,
                None => return OperationResult::Veto(VetoType::computation("Time overflow")),
            };
            OperationResult::from_literal(LiteralValue::time_with_type(
                chrono_datetime_to_semantic_time(result_dt),
                std::sync::Arc::clone(left_type),
            ))
        }

        (
            ValueKind::Time(time),
            ValueKind::Measure(_),
            ArithmeticComputation::Subtract,
        ) if right_type.is_duration_like_measure() => {
            let second = right.duration_canonical_seconds(right_type.as_ref());
            let time_aware = match semantic_time_to_chrono_datetime(time) {
                Ok(d) => d,
                Err(msg) => return OperationResult::Veto(VetoType::computation(msg)),
            };
            let duration = match seconds_to_chrono_duration(&second) {
                Ok(d) => d,
                Err(msg) => return OperationResult::Veto(VetoType::computation(msg)),
            };
            let result_dt = match time_aware.checked_sub_signed(duration) {
                Some(d) => d,
                None => return OperationResult::Veto(VetoType::computation("Time overflow")),
            };
            OperationResult::from_literal(LiteralValue::time_with_type(
                chrono_datetime_to_semantic_time(result_dt),
                std::sync::Arc::clone(left_type),
            ))
        }

        (ValueKind::Measure(_), ValueKind::Time(time), ArithmeticComputation::Add)
            if left_type.is_duration_like_measure() =>
        {
            let second = left.duration_canonical_seconds(left_type.as_ref());
            let time_aware = match semantic_time_to_chrono_datetime(time) {
                Ok(d) => d,
                Err(msg) => return OperationResult::Veto(VetoType::computation(msg)),
            };
            let duration = match seconds_to_chrono_duration(&second) {
                Ok(d) => d,
                Err(msg) => return OperationResult::Veto(VetoType::computation(msg)),
            };
            let result_dt = match time_aware.checked_add_signed(duration) {
                Some(d) => d,
                None => return OperationResult::Veto(VetoType::computation("Time overflow")),
            };
            OperationResult::from_literal(LiteralValue::time_with_type(
                chrono_datetime_to_semantic_time(result_dt),
                std::sync::Arc::clone(right_type),
            ))
        }

        (ValueKind::Time(time), ValueKind::Date(date), ArithmeticComputation::Subtract) => {
            // Time - Date: Create a datetime from the date's date components and the time's time components
            // Then subtract to get the duration
            let time_dt = match semantic_time_to_chrono_datetime(time) {
                Ok(d) => d,
                Err(msg) => return OperationResult::Veto(VetoType::computation(msg)),
            };

            // Create a datetime using the date's date components and the time's time components
            let naive_date = match NaiveDate::from_ymd_opt(date.year, date.month, date.day) {
                Some(d) => d,
                None => {
                    return OperationResult::Veto(VetoType::computation(format!(
                        "Invalid date: {}-{}-{}",
                        date.year, date.month, date.day
                    )))
                }
            };
            let naive_time = match NaiveTime::from_hms_micro_opt(
                time.hour,
                time.minute,
                time.second,
                time.microsecond,
            ) {
                Some(t) => t,
                None => {
                    return OperationResult::Veto(VetoType::computation(format!(
                        "Invalid time: {}:{}:{}.{}",
                        time.hour, time.minute, time.second, time.microsecond
                    )))
                }
            };
            let naive_dt = NaiveDateTime::new(naive_date, naive_time);

            // Use the time's timezone, or UTC if not specified
            let offset = match create_semantic_timezone_offset(&time.timezone) {
                Ok(o) => o,
                Err(msg) => return OperationResult::Veto(VetoType::computation(msg)),
            };
            let date_dt = match offset.from_local_datetime(&naive_dt).single() {
                Some(dt) => dt,
                None => {
                    return OperationResult::Veto(VetoType::computation(
                        "Ambiguous or invalid datetime for timezone",
                    ))
                }
            };

            let duration = time_dt - date_dt;
            let second = match chrono_duration_to_rational_seconds(duration) {
                Ok(value) => value,
                Err(message) => return OperationResult::Veto(VetoType::computation(message)),
            };

            OperationResult::from_literal(LiteralValue::measure(second))
        }

        _ => unreachable!(
            "BUG: time arithmetic {:?} for unsupported operand types; planning should have rejected this",
            op
        ),
    }
}

pub(crate) fn semantic_time_to_chrono_datetime(
    time: &SemanticTime,
) -> Result<DateTime<FixedOffset>, String> {
    let naive_date =
        NaiveDate::from_ymd_opt(EPOCH_YEAR, EPOCH_MONTH, EPOCH_DAY).ok_or_else(|| {
            format!(
                "Invalid epoch date: {}-{}-{}",
                EPOCH_YEAR, EPOCH_MONTH, EPOCH_DAY
            )
        })?;
    let naive_time =
        NaiveTime::from_hms_micro_opt(time.hour, time.minute, time.second, time.microsecond)
            .ok_or_else(|| {
                format!(
                    "Invalid time: {}:{}:{}.{}",
                    time.hour, time.minute, time.second, time.microsecond
                )
            })?;

    let naive_dt = NaiveDateTime::new(naive_date, naive_time);

    let offset = create_semantic_timezone_offset(&time.timezone)?;
    offset
        .from_local_datetime(&naive_dt)
        .single()
        .ok_or_else(|| "Ambiguous or invalid time for timezone".to_string())
}

fn chrono_datetime_to_semantic_time(dt: DateTime<FixedOffset>) -> SemanticTime {
    let offset_seconds = dt.offset().local_minus_utc();
    let offset_hours = (offset_seconds / SECONDS_PER_HOUR) as i8;
    let offset_minutes = ((offset_seconds.abs() % SECONDS_PER_HOUR) / SECONDS_PER_MINUTE) as u8;

    SemanticTime {
        hour: dt.hour(),
        minute: dt.minute(),
        second: dt.second(),
        microsecond: dt.nanosecond() / 1000 % 1_000_000,
        timezone: Some(SemanticTimezone {
            offset_hours,
            offset_minutes,
        }),
    }
}

// =============================================================================
// Date sugar evaluation helpers
// =============================================================================

use crate::parsing::ast::{CalendarPeriodUnit, DateCalendarKind, DateRelativeKind};

fn bool_result(b: bool) -> OperationResult {
    OperationResult::from_literal(LiteralValue {
        value: ValueKind::Boolean(b),
    })
}

/// `X in past` / `X in future`
pub fn compute_date_relative(
    kind: &DateRelativeKind,
    date: &SemanticDateTime,
    now: &SemanticDateTime,
) -> OperationResult {
    let date_chrono = match semantic_datetime_to_chrono(date) {
        Ok(dt) => dt,
        Err(msg) => return OperationResult::Veto(VetoType::computation(msg)),
    };
    let now_chrono = match semantic_datetime_to_chrono(now) {
        Ok(dt) => dt,
        Err(msg) => return OperationResult::Veto(VetoType::computation(msg)),
    };

    match kind {
        DateRelativeKind::InPast => bool_result(date_chrono < now_chrono),
        DateRelativeKind::InFuture => bool_result(date_chrono > now_chrono),
    }
}

/// Calendar period boundaries
fn calendar_boundaries(
    now: &DateTime<FixedOffset>,
    unit: &CalendarPeriodUnit,
    offset: i32,
) -> Result<(DateTime<FixedOffset>, DateTime<FixedOffset>), String> {
    let tz = *now.offset();
    match unit {
        CalendarPeriodUnit::Year => {
            let target_year = now.year() + offset;
            let start = NaiveDate::from_ymd_opt(target_year, 1, 1)
                .ok_or_else(|| format!("Invalid year: {}", target_year))?
                .and_hms_opt(0, 0, 0)
                .ok_or("Invalid start time")?;
            let end = NaiveDate::from_ymd_opt(target_year, 12, 31)
                .ok_or_else(|| format!("Invalid year end: {}", target_year))?
                .and_hms_micro_opt(23, 59, 59, 999_999)
                .ok_or("Invalid end time")?;
            let start_dt = tz
                .from_local_datetime(&start)
                .single()
                .ok_or("Ambiguous start datetime")?;
            let end_dt = tz
                .from_local_datetime(&end)
                .single()
                .ok_or("Ambiguous end datetime")?;
            Ok((start_dt, end_dt))
        }
        CalendarPeriodUnit::Month => {
            let mut target_year = now.year();
            let mut target_month = now.month() as i32 + offset;
            while target_month < 1 {
                target_month += 12;
                target_year -= 1;
            }
            while target_month > 12 {
                target_month -= 12;
                target_year += 1;
            }
            let tm = target_month as u32;
            let start = NaiveDate::from_ymd_opt(target_year, tm, 1)
                .ok_or_else(|| format!("Invalid month start: {}-{}", target_year, tm))?
                .and_hms_opt(0, 0, 0)
                .ok_or("Invalid start time")?;
            let next_month_start = if tm == 12 {
                NaiveDate::from_ymd_opt(target_year + 1, 1, 1)
            } else {
                NaiveDate::from_ymd_opt(target_year, tm + 1, 1)
            }
            .ok_or_else(|| format!("Invalid next month: {}-{}", target_year, tm + 1))?;
            let last_day = next_month_start
                .pred_opt()
                .ok_or("Invalid last day of month")?;
            let end = last_day
                .and_hms_micro_opt(23, 59, 59, 999_999)
                .ok_or("Invalid end time")?;
            let start_dt = tz
                .from_local_datetime(&start)
                .single()
                .ok_or("Ambiguous start datetime")?;
            let end_dt = tz
                .from_local_datetime(&end)
                .single()
                .ok_or("Ambiguous end datetime")?;
            Ok((start_dt, end_dt))
        }
        CalendarPeriodUnit::Week => {
            let current_monday = NaiveDate::from_isoywd_opt(
                now.iso_week().year(),
                now.iso_week().week(),
                chrono::Weekday::Mon,
            )
            .ok_or_else(|| {
                format!(
                    "Invalid ISO week: year={}, week={}",
                    now.iso_week().year(),
                    now.iso_week().week()
                )
            })?;
            let monday = current_monday + ChronoDuration::weeks(offset as i64);
            let sunday = monday + ChronoDuration::days(6);
            let start = monday.and_hms_opt(0, 0, 0).ok_or("Invalid start time")?;
            let end = sunday
                .and_hms_micro_opt(23, 59, 59, 999_999)
                .ok_or("Invalid end time")?;
            let start_dt = tz
                .from_local_datetime(&start)
                .single()
                .ok_or("Ambiguous start datetime")?;
            let end_dt = tz
                .from_local_datetime(&end)
                .single()
                .ok_or("Ambiguous end datetime")?;
            Ok((start_dt, end_dt))
        }
    }
}

/// `X in [past|future] calendar year|month|week` / `X not in calendar year|month|week`
pub fn compute_date_calendar(
    kind: &DateCalendarKind,
    unit: &CalendarPeriodUnit,
    date: &SemanticDateTime,
    now: &SemanticDateTime,
) -> OperationResult {
    let date_chrono = match semantic_datetime_to_chrono(date) {
        Ok(dt) => dt,
        Err(msg) => return OperationResult::Veto(VetoType::computation(msg)),
    };
    let now_chrono = match semantic_datetime_to_chrono(now) {
        Ok(dt) => dt,
        Err(msg) => return OperationResult::Veto(VetoType::computation(msg)),
    };

    let offset = match kind {
        DateCalendarKind::Current | DateCalendarKind::NotIn => 0,
        DateCalendarKind::Past => -1,
        DateCalendarKind::Future => 1,
    };

    let (start, end) = match calendar_boundaries(&now_chrono, unit, offset) {
        Ok(bounds) => bounds,
        Err(msg) => return OperationResult::Veto(VetoType::computation(msg)),
    };

    let in_period = date_chrono >= start && date_chrono <= end;
    let result = match kind {
        DateCalendarKind::NotIn => !in_period,
        _ => in_period,
    };
    bool_result(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn utc_datetime(y: i32, m: u32, d: u32, h: u32, min: u32, s: u32) -> SemanticDateTime {
        SemanticDateTime {
            year: y,
            month: m,
            day: d,
            hour: h,
            minute: min,
            second: s,
            microsecond: 0,
            timezone: Some(SemanticTimezone {
                offset_hours: 0,
                offset_minutes: 0,
            }),
        }
    }

    fn tz_datetime(
        (y, m, d, h, min, s, us): (i32, u32, u32, u32, u32, u32, u32),
        (tz_h, tz_m): (i8, u8),
    ) -> SemanticDateTime {
        SemanticDateTime {
            year: y,
            month: m,
            day: d,
            hour: h,
            minute: min,
            second: s,
            microsecond: us,
            timezone: Some(SemanticTimezone {
                offset_hours: tz_h,
                offset_minutes: tz_m,
            }),
        }
    }

    fn assert_true(result: &OperationResult) {
        match result {
            OperationResult::Value(v) => match &v.value {
                ValueKind::Boolean(b) => assert!(*b, "expected true, got false"),
                other => panic!("expected Boolean, got {:?}", other),
            },
            OperationResult::Veto(reason) => panic!("expected Value(true), got Veto({:?})", reason),
        }
    }

    fn assert_false(result: &OperationResult) {
        match result {
            OperationResult::Value(v) => match &v.value {
                ValueKind::Boolean(b) => assert!(!*b, "expected false, got true"),
                other => panic!("expected Boolean, got {:?}", other),
            },
            OperationResult::Veto(reason) => {
                panic!("expected Value(false), got Veto({:?})", reason)
            }
        }
    }

    // ── compute_date_relative ──────────────────────────────────────────

    #[test]
    fn in_past_date_before_now() {
        let now = utc_datetime(2026, 3, 7, 12, 0, 0);
        let date = utc_datetime(2026, 3, 1, 0, 0, 0);
        assert_true(&compute_date_relative(
            &DateRelativeKind::InPast,
            &date,
            &now,
        ));
    }

    #[test]
    fn in_past_date_equal_now_is_false() {
        let now = utc_datetime(2026, 3, 7, 12, 0, 0);
        assert_false(&compute_date_relative(
            &DateRelativeKind::InPast,
            &now,
            &now,
        ));
    }

    #[test]
    fn in_past_date_after_now() {
        let now = utc_datetime(2026, 3, 7, 12, 0, 0);
        let date = utc_datetime(2026, 4, 1, 0, 0, 0);
        assert_false(&compute_date_relative(
            &DateRelativeKind::InPast,
            &date,
            &now,
        ));
    }

    #[test]
    fn in_future_date_after_now() {
        let now = utc_datetime(2026, 3, 7, 12, 0, 0);
        let date = utc_datetime(2026, 4, 1, 0, 0, 0);
        assert_true(&compute_date_relative(
            &DateRelativeKind::InFuture,
            &date,
            &now,
        ));
    }

    #[test]
    fn in_future_date_equal_now_is_false() {
        let now = utc_datetime(2026, 3, 7, 12, 0, 0);
        assert_false(&compute_date_relative(
            &DateRelativeKind::InFuture,
            &now,
            &now,
        ));
    }

    #[test]
    fn in_future_date_before_now() {
        let now = utc_datetime(2026, 3, 7, 12, 0, 0);
        let date = utc_datetime(2026, 1, 1, 0, 0, 0);
        assert_false(&compute_date_relative(
            &DateRelativeKind::InFuture,
            &date,
            &now,
        ));
    }

    // ── compute_date_calendar ──────────────────────────────────────────

    #[test]
    fn in_calendar_year_same_year() {
        let now = utc_datetime(2026, 3, 7, 12, 0, 0);
        let date = utc_datetime(2026, 6, 15, 0, 0, 0);
        assert_true(&compute_date_calendar(
            &DateCalendarKind::Current,
            &CalendarPeriodUnit::Year,
            &date,
            &now,
        ));
    }

    #[test]
    fn in_calendar_year_different_year() {
        let now = utc_datetime(2026, 3, 7, 12, 0, 0);
        let date = utc_datetime(2025, 6, 15, 0, 0, 0);
        assert_false(&compute_date_calendar(
            &DateCalendarKind::Current,
            &CalendarPeriodUnit::Year,
            &date,
            &now,
        ));
    }

    #[test]
    fn in_calendar_year_boundary_jan_1() {
        let now = utc_datetime(2026, 3, 7, 12, 0, 0);
        let date = utc_datetime(2026, 1, 1, 0, 0, 0);
        assert_true(&compute_date_calendar(
            &DateCalendarKind::Current,
            &CalendarPeriodUnit::Year,
            &date,
            &now,
        ));
    }

    #[test]
    fn in_calendar_year_boundary_dec_31() {
        let now = utc_datetime(2026, 3, 7, 12, 0, 0);
        let date = utc_datetime(2026, 12, 31, 23, 59, 59);
        assert_true(&compute_date_calendar(
            &DateCalendarKind::Current,
            &CalendarPeriodUnit::Year,
            &date,
            &now,
        ));
    }

    #[test]
    fn in_past_calendar_year() {
        let now = utc_datetime(2026, 3, 7, 12, 0, 0);
        let date = utc_datetime(2025, 6, 15, 0, 0, 0);
        assert_true(&compute_date_calendar(
            &DateCalendarKind::Past,
            &CalendarPeriodUnit::Year,
            &date,
            &now,
        ));
    }

    #[test]
    fn in_past_calendar_year_current_year_excluded() {
        let now = utc_datetime(2026, 3, 7, 12, 0, 0);
        let date = utc_datetime(2026, 1, 15, 0, 0, 0);
        assert_false(&compute_date_calendar(
            &DateCalendarKind::Past,
            &CalendarPeriodUnit::Year,
            &date,
            &now,
        ));
    }

    #[test]
    fn in_future_calendar_year() {
        let now = utc_datetime(2026, 3, 7, 12, 0, 0);
        let date = utc_datetime(2027, 6, 15, 0, 0, 0);
        assert_true(&compute_date_calendar(
            &DateCalendarKind::Future,
            &CalendarPeriodUnit::Year,
            &date,
            &now,
        ));
    }

    #[test]
    fn in_future_calendar_year_current_year_excluded() {
        let now = utc_datetime(2026, 3, 7, 12, 0, 0);
        let date = utc_datetime(2026, 12, 31, 0, 0, 0);
        assert_false(&compute_date_calendar(
            &DateCalendarKind::Future,
            &CalendarPeriodUnit::Year,
            &date,
            &now,
        ));
    }

    #[test]
    fn not_in_calendar_year_different_year() {
        let now = utc_datetime(2026, 3, 7, 12, 0, 0);
        let date = utc_datetime(2025, 6, 15, 0, 0, 0);
        assert_true(&compute_date_calendar(
            &DateCalendarKind::NotIn,
            &CalendarPeriodUnit::Year,
            &date,
            &now,
        ));
    }

    #[test]
    fn not_in_calendar_year_same_year() {
        let now = utc_datetime(2026, 3, 7, 12, 0, 0);
        let date = utc_datetime(2026, 6, 15, 0, 0, 0);
        assert_false(&compute_date_calendar(
            &DateCalendarKind::NotIn,
            &CalendarPeriodUnit::Year,
            &date,
            &now,
        ));
    }

    #[test]
    fn in_calendar_month_same_month() {
        let now = utc_datetime(2026, 3, 7, 12, 0, 0);
        let date = utc_datetime(2026, 3, 20, 0, 0, 0);
        assert_true(&compute_date_calendar(
            &DateCalendarKind::Current,
            &CalendarPeriodUnit::Month,
            &date,
            &now,
        ));
    }

    #[test]
    fn in_calendar_month_different_month() {
        let now = utc_datetime(2026, 3, 7, 12, 0, 0);
        let date = utc_datetime(2026, 4, 1, 0, 0, 0);
        assert_false(&compute_date_calendar(
            &DateCalendarKind::Current,
            &CalendarPeriodUnit::Month,
            &date,
            &now,
        ));
    }

    #[test]
    fn in_calendar_month_boundary_first_day() {
        let now = utc_datetime(2026, 3, 7, 12, 0, 0);
        let date = utc_datetime(2026, 3, 1, 0, 0, 0);
        assert_true(&compute_date_calendar(
            &DateCalendarKind::Current,
            &CalendarPeriodUnit::Month,
            &date,
            &now,
        ));
    }

    #[test]
    fn in_calendar_month_boundary_last_day_march() {
        let now = utc_datetime(2026, 3, 7, 12, 0, 0);
        let date = utc_datetime(2026, 3, 31, 23, 59, 59);
        assert_true(&compute_date_calendar(
            &DateCalendarKind::Current,
            &CalendarPeriodUnit::Month,
            &date,
            &now,
        ));
    }

    #[test]
    fn in_calendar_month_feb_leap_year_boundary() {
        let now = utc_datetime(2024, 2, 15, 12, 0, 0);
        let date = utc_datetime(2024, 2, 29, 23, 59, 59);
        assert_true(&compute_date_calendar(
            &DateCalendarKind::Current,
            &CalendarPeriodUnit::Month,
            &date,
            &now,
        ));
    }

    #[test]
    fn in_calendar_month_feb_non_leap_year_boundary() {
        let now = utc_datetime(2025, 2, 15, 12, 0, 0);
        let date = utc_datetime(2025, 2, 28, 23, 59, 59);
        assert_true(&compute_date_calendar(
            &DateCalendarKind::Current,
            &CalendarPeriodUnit::Month,
            &date,
            &now,
        ));
    }

    #[test]
    fn in_past_calendar_month() {
        let now = utc_datetime(2026, 3, 7, 12, 0, 0);
        let date = utc_datetime(2026, 2, 15, 0, 0, 0);
        assert_true(&compute_date_calendar(
            &DateCalendarKind::Past,
            &CalendarPeriodUnit::Month,
            &date,
            &now,
        ));
    }

    #[test]
    fn in_past_calendar_month_cross_year() {
        let now = utc_datetime(2026, 1, 15, 12, 0, 0);
        let date = utc_datetime(2025, 12, 20, 0, 0, 0);
        assert_true(&compute_date_calendar(
            &DateCalendarKind::Past,
            &CalendarPeriodUnit::Month,
            &date,
            &now,
        ));
    }

    #[test]
    fn in_future_calendar_month() {
        let now = utc_datetime(2026, 3, 7, 12, 0, 0);
        let date = utc_datetime(2026, 4, 15, 0, 0, 0);
        assert_true(&compute_date_calendar(
            &DateCalendarKind::Future,
            &CalendarPeriodUnit::Month,
            &date,
            &now,
        ));
    }

    #[test]
    fn in_future_calendar_month_cross_year() {
        let now = utc_datetime(2026, 12, 15, 12, 0, 0);
        let date = utc_datetime(2027, 1, 10, 0, 0, 0);
        assert_true(&compute_date_calendar(
            &DateCalendarKind::Future,
            &CalendarPeriodUnit::Month,
            &date,
            &now,
        ));
    }

    #[test]
    fn in_calendar_week_same_week() {
        // 2026-03-07 is a Saturday (ISO week 10)
        let now = utc_datetime(2026, 3, 7, 12, 0, 0);
        // Monday of same week: 2026-03-02
        let date = utc_datetime(2026, 3, 2, 10, 0, 0);
        assert_true(&compute_date_calendar(
            &DateCalendarKind::Current,
            &CalendarPeriodUnit::Week,
            &date,
            &now,
        ));
    }

    #[test]
    fn in_calendar_week_different_week() {
        let now = utc_datetime(2026, 3, 7, 12, 0, 0);
        let date = utc_datetime(2026, 3, 15, 0, 0, 0);
        assert_false(&compute_date_calendar(
            &DateCalendarKind::Current,
            &CalendarPeriodUnit::Week,
            &date,
            &now,
        ));
    }

    #[test]
    fn in_calendar_week_sunday_boundary() {
        // 2026-03-07 is Saturday, same ISO week Mon 2026-03-02 through Sun 2026-03-08
        let now = utc_datetime(2026, 3, 7, 12, 0, 0);
        let date = utc_datetime(2026, 3, 8, 23, 59, 59);
        assert_true(&compute_date_calendar(
            &DateCalendarKind::Current,
            &CalendarPeriodUnit::Week,
            &date,
            &now,
        ));
    }

    #[test]
    fn not_in_calendar_month_different_month() {
        let now = utc_datetime(2026, 3, 7, 12, 0, 0);
        let date = utc_datetime(2026, 5, 1, 0, 0, 0);
        assert_true(&compute_date_calendar(
            &DateCalendarKind::NotIn,
            &CalendarPeriodUnit::Month,
            &date,
            &now,
        ));
    }

    #[test]
    fn not_in_calendar_month_same_month() {
        let now = utc_datetime(2026, 3, 7, 12, 0, 0);
        let date = utc_datetime(2026, 3, 15, 0, 0, 0);
        assert_false(&compute_date_calendar(
            &DateCalendarKind::NotIn,
            &CalendarPeriodUnit::Month,
            &date,
            &now,
        ));
    }

    // ── timezone-aware tests ───────────────────────────────────────────

    #[test]
    fn in_past_respects_timezone_offset() {
        // now is 2026-03-07 01:00 +02:00 = 2026-03-06 23:00 UTC
        let now = tz_datetime((2026, 3, 7, 1, 0, 0, 0), (2, 0));
        // date is 2026-03-07 00:00 UTC = 2026-03-07 00:00 +00:00
        let date = utc_datetime(2026, 3, 7, 0, 0, 0);
        // date (UTC midnight Mar 7) is AFTER now (UTC 23:00 Mar 6)
        assert_false(&compute_date_relative(
            &DateRelativeKind::InPast,
            &date,
            &now,
        ));
    }

    #[test]
    fn in_calendar_year_timezone_boundary_respects_now_tz() {
        // now is +05:00; calendar year boundary ends at 2026-12-31T23:59:59.999999 +05:00
        // = 2026-12-31T18:59:59.999999 UTC
        // date is 2026-12-31T23:59:59 UTC which is 2027-01-01T04:59:59 +05:00
        // so date is OUTSIDE the calendar year in now's timezone
        let now = tz_datetime((2026, 6, 15, 12, 0, 0, 0), (5, 0));
        let date = utc_datetime(2026, 12, 31, 23, 59, 59);
        assert_false(&compute_date_calendar(
            &DateCalendarKind::Current,
            &CalendarPeriodUnit::Year,
            &date,
            &now,
        ));
    }

    #[test]
    fn in_calendar_year_timezone_boundary_inside() {
        // now is +05:00; date is 2026-12-31T18:00 UTC = 2026-12-31T23:00 +05:00 → inside year
        let now = tz_datetime((2026, 6, 15, 12, 0, 0, 0), (5, 0));
        let date = utc_datetime(2026, 12, 31, 18, 0, 0);
        assert_true(&compute_date_calendar(
            &DateCalendarKind::Current,
            &CalendarPeriodUnit::Year,
            &date,
            &now,
        ));
    }

    // ── microsecond precision ──────────────────────────────────────────

    #[test]
    fn in_past_microsecond_precision_boundary() {
        let now = tz_datetime((2026, 3, 7, 12, 0, 0, 500_000), (0, 0));
        let date = tz_datetime((2026, 3, 7, 12, 0, 0, 499_999), (0, 0));
        // date is 1 microsecond before now
        assert_true(&compute_date_relative(
            &DateRelativeKind::InPast,
            &date,
            &now,
        ));
    }

    // ── calendar_boundaries direct tests ───────────────────────────────

    #[test]
    fn calendar_boundaries_year_covers_full_year() {
        let now = semantic_datetime_to_chrono(&utc_datetime(2026, 6, 15, 12, 0, 0)).unwrap();
        let (start, end) = calendar_boundaries(&now, &CalendarPeriodUnit::Year, 0).unwrap();
        assert_eq!(start.month(), 1);
        assert_eq!(start.day(), 1);
        assert_eq!(start.hour(), 0);
        assert_eq!(end.month(), 12);
        assert_eq!(end.day(), 31);
        assert_eq!(end.hour(), 23);
        assert_eq!(end.minute(), 59);
    }

    #[test]
    fn calendar_boundaries_month_feb_leap() {
        let now = semantic_datetime_to_chrono(&utc_datetime(2024, 2, 15, 0, 0, 0)).unwrap();
        let (start, end) = calendar_boundaries(&now, &CalendarPeriodUnit::Month, 0).unwrap();
        assert_eq!(start.day(), 1);
        assert_eq!(end.day(), 29);
    }

    #[test]
    fn calendar_boundaries_month_feb_non_leap() {
        let now = semantic_datetime_to_chrono(&utc_datetime(2025, 2, 15, 0, 0, 0)).unwrap();
        let (start, end) = calendar_boundaries(&now, &CalendarPeriodUnit::Month, 0).unwrap();
        assert_eq!(start.day(), 1);
        assert_eq!(end.day(), 28);
    }

    #[test]
    fn calendar_boundaries_week_monday_to_sunday() {
        // 2026-03-07 is a Saturday
        let now = semantic_datetime_to_chrono(&utc_datetime(2026, 3, 7, 12, 0, 0)).unwrap();
        let (start, end) = calendar_boundaries(&now, &CalendarPeriodUnit::Week, 0).unwrap();
        assert_eq!(start.weekday(), chrono::Weekday::Mon);
        assert_eq!(end.weekday(), chrono::Weekday::Sun);
    }

    #[test]
    fn calendar_boundaries_past_month_december_from_january() {
        let now = semantic_datetime_to_chrono(&utc_datetime(2026, 1, 15, 12, 0, 0)).unwrap();
        let (start, end) = calendar_boundaries(&now, &CalendarPeriodUnit::Month, -1).unwrap();
        assert_eq!(start.year(), 2025);
        assert_eq!(start.month(), 12);
        assert_eq!(start.day(), 1);
        assert_eq!(end.year(), 2025);
        assert_eq!(end.month(), 12);
        assert_eq!(end.day(), 31);
    }

    #[test]
    fn calendar_boundaries_future_month_january_from_december() {
        let now = semantic_datetime_to_chrono(&utc_datetime(2026, 12, 15, 12, 0, 0)).unwrap();
        let (start, end) = calendar_boundaries(&now, &CalendarPeriodUnit::Month, 1).unwrap();
        assert_eq!(start.year(), 2027);
        assert_eq!(start.month(), 1);
        assert_eq!(start.day(), 1);
        assert_eq!(end.year(), 2027);
        assert_eq!(end.month(), 1);
        assert_eq!(end.day(), 31);
    }

    #[test]
    fn calendar_boundaries_past_week_from_iso_week_1_rolls_to_prior_year() {
        // 2025-12-29 is Monday of ISO week 1 of 2026; now in week 1
        let now = semantic_datetime_to_chrono(&utc_datetime(2025, 12, 31, 12, 0, 0)).unwrap();
        assert_eq!(now.iso_week().week(), 1);
        let (start, end) = calendar_boundaries(&now, &CalendarPeriodUnit::Week, -1).unwrap();
        // Past week = ISO week 52 of 2025: Mon 2025-12-22 through Sun 2025-12-28
        assert_eq!(start.year(), 2025);
        assert_eq!(start.month(), 12);
        assert_eq!(start.day(), 22);
        assert_eq!(start.weekday(), chrono::Weekday::Mon);
        assert_eq!(end.day(), 28);
        assert_eq!(end.weekday(), chrono::Weekday::Sun);
    }

    #[test]
    fn calendar_boundaries_future_week_from_iso_week_52_rolls_to_next_year() {
        // 2025-12-22 is Monday of ISO week 52 of 2025
        let now = semantic_datetime_to_chrono(&utc_datetime(2025, 12, 24, 12, 0, 0)).unwrap();
        assert_eq!(now.iso_week().week(), 52);
        let (start, end) = calendar_boundaries(&now, &CalendarPeriodUnit::Week, 1).unwrap();
        // Future week = ISO week 1 of 2026: Mon 2025-12-29 through Sun 2026-01-04
        assert_eq!(start.month(), 12);
        assert_eq!(start.day(), 29);
        assert_eq!(start.weekday(), chrono::Weekday::Mon);
        assert_eq!(end.year(), 2026);
        assert_eq!(end.month(), 1);
        assert_eq!(end.day(), 4);
        assert_eq!(end.weekday(), chrono::Weekday::Sun);
    }

    #[test]
    fn calendar_boundaries_past_week_from_iso_week_53_year() {
        // 2026 has 53 ISO week. 2026-12-28 is Monday of ISO week 53.
        let now = semantic_datetime_to_chrono(&utc_datetime(2026, 12, 30, 12, 0, 0)).unwrap();
        assert_eq!(now.iso_week().week(), 53);
        let (start, end) = calendar_boundaries(&now, &CalendarPeriodUnit::Week, -1).unwrap();
        // Past week = ISO week 52: Mon 2026-12-21 through Sun 2026-12-27
        assert_eq!(start.day(), 21);
        assert_eq!(start.weekday(), chrono::Weekday::Mon);
        assert_eq!(end.day(), 27);
        assert_eq!(end.weekday(), chrono::Weekday::Sun);
    }
}
