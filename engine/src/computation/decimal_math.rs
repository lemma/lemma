//! Decimal-precision `atan`, `asin`, `acos`, `tan`.
//!
//! `atan` reduces its argument to `[0, √2−1]` before the Taylor series, so the
//! series converges in a bounded number of terms for every finite input.
//! `tan` is `sin / cos`, using `rust_decimal`'s `sin` and `cos`.
//! `sqrt`, `sin`, `cos`, `ln`, `exp` are delegated to `rust_decimal::MathematicalOps`.

use rust_decimal::Decimal;
use rust_decimal::MathematicalOps;
use std::str::FromStr;

const ATAN_SERIES_MAX_TERMS: u32 = 64;

fn half_pi() -> Decimal {
    Decimal::from_str("1.5707963267948966192313216916").expect("BUG: π/2 constant failed to parse")
}

fn quarter_pi() -> Decimal {
    half_pi() / Decimal::from(2)
}

fn tan_pi_8() -> Decimal {
    Decimal::from(2).sqrt().expect("BUG: sqrt(2) must exist") - Decimal::ONE
}

/// atan(x) for any finite `x`.
///
/// Odd symmetry maps `x` to `|x|`. For `|x| > 1`, `atan(x) = π/2 − atan(1/x)`.
/// The remaining value in `[0, 1]` is reduced by `atan(y) = π/4 + atan((y−1)/(y+1))`
/// when `y > √2−1`, so the series only ever sees `|argument| <= √2−1`.
pub fn decimal_atan(x: Decimal) -> Decimal {
    if x < Decimal::ZERO {
        return -atan_non_negative(-x);
    }
    atan_non_negative(x)
}

fn atan_non_negative(x: Decimal) -> Decimal {
    if x > Decimal::ONE {
        return half_pi() - atan_unit_interval(Decimal::ONE / x);
    }
    atan_unit_interval(x)
}

fn atan_unit_interval(y: Decimal) -> Decimal {
    if y > tan_pi_8() {
        quarter_pi() + atan_series((y - Decimal::ONE) / (y + Decimal::ONE))
    } else {
        atan_series(y)
    }
}

/// Taylor series `y − y³/3 + y⁵/5 − …`, requires `|y| <= √2−1`.
fn atan_series(y: Decimal) -> Decimal {
    assert!(
        y.abs() <= tan_pi_8(),
        "BUG: atan series argument not reduced to |y| <= sqrt(2)-1"
    );
    let y2 = y * y;
    let mut term = y;
    let mut result = y;
    let mut n: i64 = 1;
    for _ in 0..ATAN_SERIES_MAX_TERMS {
        n += 2;
        term = -(term * y2);
        let contribution = term / Decimal::from(n);
        let previous = result;
        result += contribution;
        if result == previous {
            return result;
        }
    }
    panic!(
        "BUG: atan series did not converge within {ATAN_SERIES_MAX_TERMS} terms after reduction"
    );
}

/// asin(x) = atan(x / sqrt(1 - x²)) for |x| < 1, ±π/2 for |x| = 1.
/// Returns None for |x| > 1 (domain error — legitimate Veto condition).
pub fn decimal_asin(x: Decimal) -> Option<Decimal> {
    let hp = half_pi();
    if x == Decimal::ONE {
        return Some(hp);
    }
    if x == Decimal::NEGATIVE_ONE {
        return Some(-hp);
    }
    if x.abs() > Decimal::ONE {
        return None;
    }
    let one_minus_x2 = Decimal::ONE.checked_sub(x.checked_mul(x)?)?;
    let denom = one_minus_x2.sqrt()?;
    if denom.is_zero() {
        return None;
    }
    Some(decimal_atan(x.checked_div(denom)?))
}

/// acos(x) = π/2 - asin(x).
/// Returns None for |x| > 1 (domain error — legitimate Veto condition).
pub fn decimal_acos(x: Decimal) -> Option<Decimal> {
    let asin_x = decimal_asin(x)?;
    half_pi().checked_sub(asin_x)
}

pub fn decimal_tan(x: Decimal) -> Option<Decimal> {
    let cos_x = x.checked_cos()?;
    if cos_x.is_zero() {
        return None;
    }
    x.checked_sin()?.checked_div(cos_x)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(literal: &str) -> Decimal {
        Decimal::from_str(literal).expect("BUG: test literal must parse")
    }

    fn tol() -> Decimal {
        Decimal::new(1, 26)
    }

    fn assert_close(actual: Decimal, expected: Decimal, tolerance: Decimal) {
        let diff = (actual - expected).abs();
        assert!(
            diff <= tolerance,
            "expected ~{expected} (±{tolerance}), got {actual} (diff {diff})"
        );
    }

    #[test]
    fn atan_zero_is_zero() {
        assert_eq!(decimal_atan(Decimal::ZERO), Decimal::ZERO);
    }

    #[test]
    fn atan_one_is_quarter_pi() {
        assert_close(decimal_atan(Decimal::ONE), quarter_pi(), tol());
    }

    #[test]
    fn atan_minus_one_is_minus_quarter_pi() {
        assert_close(decimal_atan(Decimal::NEGATIVE_ONE), -quarter_pi(), tol());
    }

    #[test]
    fn atan_at_reduction_boundary_is_eighth_pi() {
        assert_close(
            decimal_atan(tan_pi_8()),
            quarter_pi() / Decimal::from(2),
            tol(),
        );
    }

    #[test]
    fn atan_is_odd() {
        for x in [d("0.3"), d("0.9"), d("1.7"), d("42")] {
            assert_close(decimal_atan(-x), -decimal_atan(x), tol());
        }
    }

    #[test]
    fn atan_reciprocal_identity_for_positive_inputs() {
        for x in [d("0.3"), d("0.9"), d("1.7"), d("5"), d("10000000000")] {
            assert_close(
                decimal_atan(x) + decimal_atan(Decimal::ONE / x),
                half_pi(),
                tol(),
            );
        }
    }

    #[test]
    fn atan_large_magnitude_approaches_half_pi() {
        assert_close(
            decimal_atan(d("100000000000000000000")),
            half_pi(),
            d("0.0000000000000000001"),
        );
        assert_close(decimal_atan(Decimal::MAX), half_pi(), tol());
        assert_close(decimal_atan(Decimal::MIN), -half_pi(), tol());
    }

    #[test]
    fn atan_round_trips_through_tan() {
        let round_trip_tol = d("0.000000000000000000000001");
        for x in [
            d("0.1"),
            d("0.5"),
            d("0.9"),
            d("0.9999"),
            d("1"),
            d("2"),
            d("7.5"),
        ] {
            let angle = decimal_atan(x);
            let back = decimal_tan(angle).expect("BUG: tan of atan result must exist");
            assert_close(back, x, round_trip_tol);
        }
    }

    #[test]
    fn asin_of_one_over_root_two_is_quarter_pi() {
        let half_root_two = d("0.7071067811865475244008443621");
        assert_close(
            decimal_asin(half_root_two).expect("in domain"),
            quarter_pi(),
            tol(),
        );
    }

    #[test]
    fn acos_of_one_over_root_two_is_quarter_pi() {
        let half_root_two = d("0.7071067811865475244008443621");
        assert_close(
            decimal_acos(half_root_two).expect("in domain"),
            quarter_pi(),
            tol(),
        );
    }

    #[test]
    fn asin_and_acos_outside_domain_are_none() {
        assert_eq!(decimal_asin(d("1.5")), None);
        assert_eq!(decimal_asin(d("-1.5")), None);
        assert_eq!(decimal_acos(d("1.5")), None);
    }

    fn assert_relative(actual: Decimal, expected: Decimal, relative_tolerance: Decimal) {
        let diff = (actual - expected).abs();
        let bound = expected.abs() * relative_tolerance;
        assert!(
            diff <= bound,
            "expected ~{expected} (relative ±{relative_tolerance}), got {actual} (diff {diff})"
        );
    }

    #[test]
    fn tan_zero_is_zero() {
        assert_eq!(decimal_tan(Decimal::ZERO), Some(Decimal::ZERO));
    }

    #[test]
    fn tan_quarter_pi_is_one() {
        assert_close(
            decimal_tan(quarter_pi()).expect("in domain"),
            Decimal::ONE,
            tol(),
        );
    }

    #[test]
    fn tan_is_odd() {
        for x in [d("0.3"), d("1.2"), d("2.5")] {
            let negative = decimal_tan(-x).expect("in domain");
            let positive = decimal_tan(x).expect("in domain");
            assert_close(negative, -positive, tol());
        }
    }

    #[test]
    fn tan_has_period_pi() {
        for x in [d("0.3"), d("1.2")] {
            let shifted = decimal_tan(x + Decimal::PI).expect("in domain");
            assert_close(
                shifted,
                decimal_tan(x).expect("in domain"),
                d("0.0000000000000000000000001"),
            );
        }
    }

    #[test]
    fn tan_matches_reference_values() {
        let cases = [
            ("0.1", "0.1003346720854505450580800458"),
            ("0.5", "0.5463024898437905132551794658"),
            ("1", "1.5574077246549022305069748075"),
            ("1.5", "14.1014199471717193876460836520"),
            ("-0.7", "-0.8422883804630794481281350022"),
            ("2.5", "-0.7470222972386602793553526878"),
            ("3.0", "-0.1425465430742778052956354105"),
        ];
        for (x, expected) in cases {
            let actual = decimal_tan(d(x)).expect("in domain");
            assert_relative(actual, d(expected), d("0.00000000000000000000000001"));
        }
    }

    #[test]
    fn tan_near_half_pi_matches_reference() {
        let x = half_pi() - d("0.001");
        assert_relative(
            decimal_tan(x).expect("in domain"),
            d("999.9996666666444444423280421164"),
            d("0.000000000000000000000001"),
        );
    }

    #[test]
    fn tan_at_half_pi_is_none() {
        assert_eq!(decimal_tan(half_pi()), None);
    }
}
