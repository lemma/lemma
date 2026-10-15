//! Exact rational arithmetic with an inline i128 fast path.
//!
//! Small numerator/denominator pairs (both fit `i128`) use zero-allocation
//! native arithmetic. On overflow the value promotes to heap-allocated
//! [`BigInt`]. All constructors produce reduced form; callers never see
//! unreduced fractions.
//!
//! Input literals parse through [`crate::literals::NumberLiteral`] (`Decimal::from_str`).
//! API decimal conversion uses [`RationalInteger::try_to_decimal`], enforcing the same
//! [`Decimal::MAX_SCALE`] digit limit.

use rust_decimal::Decimal;
use rust_decimal::MathematicalOps;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::fmt;
use std::str::FromStr;
use std::sync::LazyLock;

use crate::computation::bigint::AllocError;
pub(crate) use crate::computation::bigint::{BigInt, BigUint};
use crate::limits::MAX_RATIONAL_BITS;

/// Digit limit for ℚ → [`Decimal`] API conversion.
///
/// Matches [`Decimal::MAX_SCALE`] and [`crate::literals::NumberLiteral`] input parsing.
const DECIMAL_OUTPUT_DIGIT_LIMIT: usize = Decimal::MAX_SCALE as usize;

static DECIMAL_MAX_RATIONAL: LazyLock<RationalInteger> =
    LazyLock::new(|| decimal_to_rational_unchecked(Decimal::MAX.normalize()));

static DECIMAL_MIN_RATIONAL: LazyLock<RationalInteger> =
    LazyLock::new(|| decimal_to_rational_unchecked(Decimal::MIN.normalize()));

fn decimal_to_rational_unchecked(decimal: Decimal) -> RationalInteger {
    decimal_to_rational(decimal).expect("BUG: Decimal::MAX/MIN must convert to rational")
}

// ---------------------------------------------------------------------------
// Small-integer GCD (binary GCD on u128, no allocation)
// ---------------------------------------------------------------------------

fn gcd_u128(mut a: u128, mut b: u128) -> u128 {
    if a == 0 {
        return b;
    }
    if b == 0 {
        return a;
    }
    let shift = (a | b).trailing_zeros();
    a >>= a.trailing_zeros();
    loop {
        b >>= b.trailing_zeros();
        if a > b {
            std::mem::swap(&mut a, &mut b);
        }
        b -= a;
        if b == 0 {
            return a << shift;
        }
    }
}

/// Reduce `(numer, denom)` in-place. `denom` must be > 0 on entry.
fn reduce_small(numer: i128, denom: i128) -> (i128, i128) {
    debug_assert!(denom > 0);
    if numer == 0 {
        return (0, 1);
    }
    let g = i128::try_from(gcd_u128(numer.unsigned_abs(), denom.unsigned_abs()))
        .expect("BUG: gcd divides the positive i128 denominator");
    (numer / g, denom / g)
}

// ---------------------------------------------------------------------------
// RationalInteger
// ---------------------------------------------------------------------------

/// Serialized as-is: `Small` is two varints, `Big` two bignums. Every constructor
/// produces the canonical form (positive denominator, gcd 1, `Big` only when the
/// pair does not fit `i128`), and deserialization accepts nothing else.
#[derive(Clone, Debug, Serialize, Deserialize)]
enum Repr {
    Small { numer: i128, denom: i128 },
    Big { numer: BigInt, denom: BigInt },
}

#[derive(Clone, Debug)]
pub struct RationalInteger {
    repr: Repr,
}

impl Serialize for RationalInteger {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.repr.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for RationalInteger {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let repr = Repr::deserialize(deserializer)?;
        Self::from_canonical_repr(repr).map_err(serde::de::Error::custom)
    }
}

impl RationalInteger {
    /// Accept a decoded `Repr` only if it is the form the constructors would have
    /// produced; anything else is corrupt or foreign input.
    fn from_canonical_repr(repr: Repr) -> Result<Self, String> {
        match repr {
            Repr::Small { numer, denom } => {
                if denom <= 0 {
                    return Err(format!(
                        "RationalInteger denominator must be positive, got {denom}"
                    ));
                }
                if reduce_small(numer, denom) != (numer, denom) {
                    return Err(format!("RationalInteger {numer}/{denom} is not reduced"));
                }
                Ok(Self::from_reduced_small(numer, denom))
            }
            Repr::Big { numer, denom } => {
                if !denom.is_positive() {
                    return Err(format!(
                        "RationalInteger denominator must be positive, got {denom}"
                    ));
                }
                if numer.to_i128().is_some() && denom.to_i128().is_some() {
                    return Err(format!(
                        "RationalInteger {numer}/{denom} fits i128 and must be encoded Small"
                    ));
                }
                let candidate = Self {
                    repr: Repr::Big { numer, denom },
                };
                let reduced = Self::try_reduce_ref(&candidate)
                    .map_err(|failure| format!("invalid RationalInteger: {failure}"))?;
                if reduced != candidate {
                    return Err(format!(
                        "RationalInteger {}/{} is not reduced",
                        candidate.numer_to_string(),
                        candidate.denom_to_string()
                    ));
                }
                Ok(candidate)
            }
        }
    }
}

impl RationalInteger {
    fn to_big_numer(&self) -> BigInt {
        match &self.repr {
            Repr::Small { numer, .. } => BigInt::from_i128(*numer),
            Repr::Big { numer, .. } => numer.clone(),
        }
    }

    fn to_big_denom(&self) -> BigInt {
        match &self.repr {
            Repr::Small { denom, .. } => BigInt::from_i128(*denom),
            Repr::Big { denom, .. } => denom.clone(),
        }
    }

    /// Promote to Big representation for fallback arithmetic.
    fn as_big_pair(&self) -> (BigInt, BigInt) {
        (self.to_big_numer(), self.to_big_denom())
    }
}

impl RationalInteger {
    // -- Construction --

    /// Construct from already-reduced i128 pair. `denom` MUST be > 0 and gcd == 1.
    fn from_reduced_small(numer: i128, denom: i128) -> Self {
        debug_assert!(denom > 0);
        Self {
            repr: Repr::Small { numer, denom },
        }
    }

    /// Construct from i128 pair, reducing.
    fn from_small(numer: i128, denom: i128) -> Result<Self, NumericFailure> {
        if denom == 0 {
            return Err(NumericFailure::DivisionByZero);
        }
        let (n, d) = if denom < 0 {
            match (numer.checked_neg(), denom.checked_neg()) {
                (Some(n), Some(d)) => (n, d),
                _ => return Self::reduce_big(BigInt::from_i128(numer), BigInt::from_i128(denom)),
            }
        } else {
            (numer, denom)
        };
        let (rn, rd) = reduce_small(n, d);
        Ok(Self::from_reduced_small(rn, rd))
    }

    /// Construct without reducing. Callers guarantee `gcd(numer, denom) == 1`; `denom`
    /// must be positive.
    fn from_coprime(numer: BigInt, denom: BigInt) -> Self {
        assert!(
            denom.is_positive(),
            "BUG: from_coprime requires a positive denominator, got {denom}"
        );
        match (numer.to_i128(), denom.to_i128()) {
            (Some(n), Some(d)) => Self::from_reduced_small(n, d),
            _ => Self {
                repr: Repr::Big { numer, denom },
            },
        }
    }

    /// `(numer, denom)` of `1 / self` with a positive denominator. Reduced input stays reduced.
    fn try_reciprocal_pair(&self) -> Result<(BigInt, BigInt), NumericFailure> {
        if self.numer_is_zero() {
            return Err(NumericFailure::DivisionByZero);
        }
        let (numer, denom) = self.as_big_pair();
        if numer.is_negative() {
            Ok((denom.try_neg()?, numer.try_neg()?))
        } else {
            Ok((denom, numer))
        }
    }

    fn try_reciprocal(&self) -> Result<Self, NumericFailure> {
        let (numer, denom) = self.try_reciprocal_pair()?;
        Ok(Self::from_coprime(numer, denom))
    }

    pub fn try_new(numer: BigInt, denom: BigInt) -> Result<Self, NumericFailure> {
        if denom.is_zero() {
            return Err(NumericFailure::DivisionByZero);
        }
        // Try small path first
        if let (Some(n), Some(d)) = (numer.to_i128(), denom.to_i128()) {
            return Self::from_small(n, d);
        }
        Self::reduce_big(numer, denom)
    }

    pub fn from_i64_pair(numer: i64, denom: i64) -> Self {
        Self::from_small(i128::from(numer), i128::from(denom))
            .expect("BUG: i64 rational reduce cannot fail")
    }

    /// Construct from string-parsed BigInts (used by `SerializedFactor` round-trip).
    pub fn from_bigint_strings(numer_str: &str, denom_str: &str) -> Result<Self, NumericFailure> {
        let n = BigInt::try_from_str_radix(numer_str, 10).map_err(NumericFailure::from)?;
        let d = BigInt::try_from_str_radix(denom_str, 10).map_err(NumericFailure::from)?;
        Self::try_new(n, d)
    }

    // -- Predicates --

    pub fn is_integer(&self) -> bool {
        match &self.repr {
            Repr::Small { denom, .. } => *denom == 1,
            Repr::Big { denom, .. } => *denom == BigInt::one(),
        }
    }

    pub fn numer_is_zero(&self) -> bool {
        match &self.repr {
            Repr::Small { numer, .. } => *numer == 0,
            Repr::Big { numer, .. } => numer.is_zero(),
        }
    }

    pub fn numer_is_positive(&self) -> bool {
        match &self.repr {
            Repr::Small { numer, .. } => *numer > 0,
            Repr::Big { numer, .. } => numer.is_positive(),
        }
    }

    pub fn numer_is_negative(&self) -> bool {
        match &self.repr {
            Repr::Small { numer, .. } => *numer < 0,
            Repr::Big { numer, .. } => numer.is_negative(),
        }
    }

    // -- Conversions out --

    pub fn numer_to_i32(&self) -> Option<i32> {
        match &self.repr {
            Repr::Small { numer, .. } => i32::try_from(*numer).ok(),
            Repr::Big { numer, .. } => numer.to_i32(),
        }
    }

    pub fn numer_to_i128(&self) -> Option<i128> {
        match &self.repr {
            Repr::Small { numer, .. } => Some(*numer),
            Repr::Big { numer, .. } => numer.to_i128(),
        }
    }

    pub fn numer_to_u8(&self) -> Option<u8> {
        match &self.repr {
            Repr::Small { numer, .. } => u8::try_from(*numer).ok(),
            Repr::Big { numer, .. } => numer.to_i32().and_then(|v| u8::try_from(v).ok()),
        }
    }

    pub fn numer_to_usize(&self) -> Option<usize> {
        match &self.repr {
            Repr::Small { numer, .. } => usize::try_from(*numer).ok(),
            Repr::Big { numer, .. } => numer.to_usize(),
        }
    }

    pub fn numer_to_string(&self) -> String {
        match &self.repr {
            Repr::Small { numer, .. } => numer.to_string(),
            Repr::Big { numer, .. } => numer.to_string(),
        }
    }

    pub fn denom_to_string(&self) -> String {
        match &self.repr {
            Repr::Small { denom, .. } => denom.to_string(),
            Repr::Big { denom, .. } => denom.to_string(),
        }
    }

    /// Exact bit length of the numerator's magnitude; 0 for zero.
    pub fn numer_magnitude_bits(&self) -> u64 {
        match &self.repr {
            Repr::Small { numer, .. } => {
                if *numer == 0 {
                    0
                } else {
                    128 - numer.unsigned_abs().leading_zeros() as u64
                }
            }
            Repr::Big { numer, .. } => numer.magnitude().bits(),
        }
    }

    /// Exact bit length of the denominator's magnitude.
    pub fn denom_magnitude_bits(&self) -> u64 {
        match &self.repr {
            Repr::Small { denom, .. } => {
                if *denom == 0 {
                    0
                } else {
                    128 - denom.unsigned_abs().leading_zeros() as u64
                }
            }
            Repr::Big { denom, .. } => denom.magnitude().bits(),
        }
    }

    // -- Reduction --

    pub fn try_reduce_ref(rational: &RationalInteger) -> Result<Self, NumericFailure> {
        match &rational.repr {
            Repr::Small { numer, denom } => Self::from_small(*numer, *denom),
            Repr::Big { numer, denom } => Self::reduce_big(numer.try_clone()?, denom.try_clone()?),
        }
    }

    /// Reduce a BigInt pair: positive denominator, gcd 1, shrunk to Small when both fit i128.
    fn reduce_big(numer: BigInt, denom: BigInt) -> Result<Self, NumericFailure> {
        if denom.is_zero() {
            return Err(NumericFailure::DivisionByZero);
        }
        let (n, d) = if denom.is_negative() {
            (numer.try_neg()?, denom.try_neg()?)
        } else {
            (numer, denom)
        };
        if n.is_zero() {
            return Ok(Self::from_reduced_small(0, 1));
        }
        let gcd = n.try_abs()?.try_gcd(&d)?;
        let n = n.try_div_trunc(&gcd)?;
        let d = d.try_div_trunc(&gcd)?;
        if let (Some(sn), Some(sd)) = (n.to_i128(), d.to_i128()) {
            Ok(Self::from_reduced_small(sn, sd))
        } else {
            Ok(Self {
                repr: Repr::Big { numer: n, denom: d },
            })
        }
    }

    pub fn try_reduce(self) -> Result<Self, NumericFailure> {
        Self::try_reduce_ref(&self)
    }

    // -- Comparison --

    pub fn try_cmp(&self, other: &Self) -> Result<std::cmp::Ordering, NumericFailure> {
        match (&self.repr, &other.repr) {
            (
                Repr::Small {
                    numer: ln,
                    denom: ld,
                },
                Repr::Small {
                    numer: rn,
                    denom: rd,
                },
            ) => {
                // cross-multiply: ln * rd  vs  rn * ld
                // Use i128 checked to avoid overflow
                if let (Some(left), Some(right)) = (ln.checked_mul(*rd), rn.checked_mul(*ld)) {
                    return Ok(left.cmp(&right));
                }
                // Fallback to BigInt
                let left = BigInt::from_i128(*ln).try_mul(&BigInt::from_i128(*rd))?;
                let right = BigInt::from_i128(*rn).try_mul(&BigInt::from_i128(*ld))?;
                Ok(left.cmp(&right))
            }
            _ => {
                let left = self.to_big_numer().try_mul(&other.to_big_denom())?;
                let right = other.to_big_numer().try_mul(&self.to_big_denom())?;
                Ok(left.cmp(&right))
            }
        }
    }

    // -- Decimal conversion --

    /// Round to [`Decimal::MAX_SCALE`] precision.
    ///
    /// Returns [`NumericFailure::Overflow`] only when `|self| > Decimal::MAX`.
    ///
    /// All `RationalInteger` values are constructed in reduced form, so no
    /// re-reduction is needed here.
    pub fn try_to_decimal(&self) -> Result<Decimal, NumericFailure> {
        if self.numer_is_zero() {
            return Ok(Decimal::ZERO);
        }

        if self.try_cmp(&DECIMAL_MAX_RATIONAL)? == std::cmp::Ordering::Greater {
            return Err(NumericFailure::Overflow);
        }
        if self.try_cmp(&DECIMAL_MIN_RATIONAL)? == std::cmp::Ordering::Less {
            return Err(NumericFailure::Overflow);
        }

        // For small integers with denom==1, fast path via i128 → string → Decimal
        if let Repr::Small { numer, denom: 1 } = &self.repr {
            return Decimal::from_str(&numer.to_string()).map_err(|_| NumericFailure::Overflow);
        }

        let negative = self.numer_is_negative();
        let abs_numer = self.to_big_numer().try_abs()?;
        let abs_denom = self.to_big_denom().try_abs()?;

        let (int_quotient, mut remainder) = abs_numer.try_div_rem(&abs_denom)?;

        let mut digit_string = if int_quotient.is_zero() {
            String::from("0")
        } else {
            int_quotient.to_string()
        };

        if !remainder.is_zero() {
            digit_string.push('.');
            let integer_significant_digits = if int_quotient.is_zero() {
                0
            } else {
                digit_string.len()
            };

            if int_quotient.is_zero() {
                let mut significant_digits = 0usize;
                while !remainder.is_zero() && significant_digits < DECIMAL_OUTPUT_DIGIT_LIMIT + 1 {
                    remainder = remainder.try_mul(&BigInt::from_i64(10))?;
                    let (digit_quotient, new_remainder) = remainder.try_div_rem(&abs_denom)?;
                    let digit = u8::try_from(
                        digit_quotient
                            .to_i32()
                            .expect("BUG: fractional digit quotient must fit i32"),
                    )
                    .expect("BUG: fractional digit must be 0-9");
                    digit_string.push(char::from(b'0' + digit));
                    remainder = new_remainder;
                    if digit != 0 || significant_digits > 0 {
                        significant_digits += 1;
                    }
                }
            } else {
                let max_fractional_digits = DECIMAL_OUTPUT_DIGIT_LIMIT
                    .saturating_sub(integer_significant_digits)
                    .saturating_add(1);
                let mut fractional_count = 0usize;
                while !remainder.is_zero() && fractional_count < max_fractional_digits {
                    remainder = remainder.try_mul(&BigInt::from_i64(10))?;
                    let (digit_quotient, new_remainder) = remainder.try_div_rem(&abs_denom)?;
                    let digit = u8::try_from(
                        digit_quotient
                            .to_i32()
                            .expect("BUG: fractional digit quotient must fit i32"),
                    )
                    .expect("BUG: fractional digit must be 0-9");
                    digit_string.push(char::from(b'0' + digit));
                    remainder = new_remainder;
                    fractional_count += 1;
                }
            }
        }

        if negative {
            digit_string.insert(0, '-');
        }

        Decimal::from_str(&digit_string).map_err(|_| NumericFailure::Overflow)
    }

    pub fn try_to_decimal_string(&self) -> Result<String, NumericFailure> {
        self.try_to_decimal()
            .map(|decimal| decimal_to_display_str(&decimal))
    }

    /// Human-readable decimal when `try_to_decimal` succeeds; fraction string on magnitude overflow.
    pub fn display_str(&self) -> String {
        match self.try_to_decimal() {
            Ok(decimal) => decimal_to_display_str(&decimal),
            Err(NumericFailure::Overflow) => rational_fraction_str(self),
            Err(NumericFailure::OutOfMemory) => rational_fraction_str(self),
            Err(NumericFailure::DivisionByZero) => {
                unreachable!("BUG: display_str on reduced rational with zero denominator")
            }
            Err(NumericFailure::Irrational) => {
                unreachable!("BUG: display_str does not perform irrational operations")
            }
            Err(NumericFailure::MagnitudeLimit) => {
                unreachable!("BUG: try_to_decimal does not apply the arithmetic magnitude limit")
            }
        }
    }
}

// -- Eq / Hash / Ord --

impl PartialEq for RationalInteger {
    fn eq(&self, other: &Self) -> bool {
        match (&self.repr, &other.repr) {
            (
                Repr::Small {
                    numer: ln,
                    denom: ld,
                },
                Repr::Small {
                    numer: rn,
                    denom: rd,
                },
            ) => ln == rn && ld == rd,
            _ => {
                self.to_big_numer() == other.to_big_numer()
                    && self.to_big_denom() == other.to_big_denom()
            }
        }
    }
}

impl Eq for RationalInteger {}

impl std::hash::Hash for RationalInteger {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        match &self.repr {
            Repr::Small { numer, denom } => {
                numer.hash(state);
                denom.hash(state);
            }
            Repr::Big { numer, denom } => {
                // Normalize to i128 if possible so Small==Big hashing agrees
                if let (Some(n), Some(d)) = (numer.to_i128(), denom.to_i128()) {
                    n.hash(state);
                    d.hash(state);
                } else {
                    numer.hash(state);
                    denom.hash(state);
                }
            }
        }
    }
}

impl fmt::Display for RationalInteger {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.display_str())
    }
}

// ---------------------------------------------------------------------------
// NumericFailure
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NumericFailure {
    DivisionByZero,
    Overflow,
    OutOfMemory,
    /// Arithmetic result exceeds [`MAX_RATIONAL_BITS`].
    MagnitudeLimit,
    Irrational,
}

impl fmt::Display for NumericFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            NumericFailure::DivisionByZero => {
                formatter.write_str("Division by zero is not allowed.")
            }
            NumericFailure::Overflow => formatter.write_str("The numeric result overflowed."),
            NumericFailure::OutOfMemory => {
                formatter.write_str("The calculation ran out of memory.")
            }
            NumericFailure::MagnitudeLimit => write!(
                formatter,
                "The numeric result exceeds the maximum size of {MAX_RATIONAL_BITS} bits."
            ),
            NumericFailure::Irrational => formatter
                .write_str("The numeric result is irrational and cannot be represented exactly."),
        }
    }
}

impl From<AllocError> for NumericFailure {
    fn from(_: AllocError) -> Self {
        NumericFailure::OutOfMemory
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NumericOperation {
    Add,
    Subtract,
    Multiply,
    Divide,
    Modulo,
    Power,
}

// ---------------------------------------------------------------------------
// Free constructors
// ---------------------------------------------------------------------------

pub fn rational_one() -> RationalInteger {
    rational_new(1, 1)
}

pub fn rational_zero() -> RationalInteger {
    rational_new(0, 1)
}

pub fn rational_new(numer: i64, denom: i64) -> RationalInteger {
    RationalInteger::from_i64_pair(numer, denom)
}

pub fn try_rational_new(numer: BigInt, denom: BigInt) -> Result<RationalInteger, NumericFailure> {
    RationalInteger::try_new(numer, denom)
}

pub fn rational_is_zero(rational: &RationalInteger) -> bool {
    rational.numer_is_zero()
}

pub fn rational_abs(rational: &RationalInteger) -> Result<RationalInteger, NumericFailure> {
    match &rational.repr {
        Repr::Small { numer, denom } => match numer.checked_abs() {
            Some(n) => Ok(RationalInteger::from_reduced_small(n, *denom)),
            None => try_rational_new(
                BigInt::from_i128(*numer).try_abs()?,
                BigInt::from_i128(*denom),
            ),
        },
        Repr::Big { numer, denom } => {
            let n = numer.try_abs()?;
            let d = denom.try_clone()?;
            Ok(RationalInteger {
                repr: Repr::Big { numer: n, denom: d },
            })
        }
    }
}

pub fn rational_trunc(rational: &RationalInteger) -> Result<RationalInteger, NumericFailure> {
    match &rational.repr {
        Repr::Small { numer, denom } => Ok(RationalInteger::from_reduced_small(numer / denom, 1)),
        Repr::Big { numer, denom } => {
            let truncated = numer.try_div_trunc(denom)?;
            try_rational_new(truncated, BigInt::one())
        }
    }
}

pub fn rational_floor(rational: &RationalInteger) -> Result<RationalInteger, NumericFailure> {
    let truncated = rational_trunc(rational)?;
    if rational.numer_is_negative() && !rational.is_integer() {
        try_sub(&truncated, &rational_one())
    } else {
        Ok(truncated)
    }
}

pub fn rational_ceil(rational: &RationalInteger) -> Result<RationalInteger, NumericFailure> {
    let truncated = rational_trunc(rational)?;
    if !rational.numer_is_negative() && !rational.is_integer() {
        try_add(&truncated, &rational_one())
    } else {
        Ok(truncated)
    }
}

/// Nearest integer; a fraction of exactly one half rounds to the even integer.
pub fn rational_round(rational: &RationalInteger) -> Result<RationalInteger, NumericFailure> {
    let floor = rational_floor(rational)?;
    let fraction = try_sub(rational, &floor)?;
    match fraction.try_cmp(&rational_new(1, 2))? {
        std::cmp::Ordering::Less => Ok(floor),
        std::cmp::Ordering::Greater => try_add(&floor, &rational_one()),
        std::cmp::Ordering::Equal => {
            if try_div(&floor, &rational_new(2, 1))?.is_integer() {
                Ok(floor)
            } else {
                try_add(&floor, &rational_one())
            }
        }
    }
}

pub fn decimal_to_rational(decimal: Decimal) -> Result<RationalInteger, NumericFailure> {
    let mantissa = decimal.mantissa();
    if mantissa == 0 {
        return Ok(rational_new(0, 1));
    }
    let scale = decimal.scale();
    if scale == 0 {
        return RationalInteger::from_small(mantissa, 1);
    }
    // 10^scale — try i128 first (10^38 fits in i128)
    if scale <= 38 {
        if let Some(denom) = 10i128.checked_pow(scale) {
            return RationalInteger::from_small(mantissa, denom);
        }
    }
    // Fallback to BigInt
    let mut denominator = BigInt::one();
    for _ in 0..scale {
        denominator = denominator
            .try_mul(&BigInt::from_i64(10))
            .map_err(NumericFailure::from)?;
    }
    try_rational_new(BigInt::from_i128(mantissa), denominator)
}

pub(crate) fn decimal_to_display_str(decimal: &Decimal) -> String {
    let normalized = decimal.normalize();
    if normalized.fract().is_zero() {
        normalized.trunc().to_string()
    } else {
        normalized.to_string()
    }
}

fn rational_fraction_str(rational: &RationalInteger) -> String {
    if rational.is_integer() {
        rational.numer_to_string()
    } else {
        format!(
            "{}/{}",
            rational.numer_to_string(),
            rational.denom_to_string()
        )
    }
}

// ---------------------------------------------------------------------------
// Arithmetic operations
// ---------------------------------------------------------------------------

/// Exact arithmetic for Lemma operators. Results beyond [`MAX_RATIONAL_BITS`] fail with
/// [`NumericFailure::MagnitudeLimit`]; the bare `try_*` helpers are unbounded and reserved
/// for exact scaling by planned constants.
pub fn rational_operation(
    left: &RationalInteger,
    operation: NumericOperation,
    right: &RationalInteger,
) -> Result<RationalInteger, NumericFailure> {
    let result = rational_operation_unbounded(left, operation, right)?;
    if result.numer_magnitude_bits() > MAX_RATIONAL_BITS
        || result.denom_magnitude_bits() > MAX_RATIONAL_BITS
    {
        return Err(NumericFailure::MagnitudeLimit);
    }
    Ok(result)
}

fn rational_operation_unbounded(
    left: &RationalInteger,
    operation: NumericOperation,
    right: &RationalInteger,
) -> Result<RationalInteger, NumericFailure> {
    match operation {
        NumericOperation::Add => try_add(left, right),
        NumericOperation::Subtract => try_sub(left, right),
        NumericOperation::Multiply => try_mul(left, right),
        NumericOperation::Divide => {
            if rational_is_zero(right) {
                return Err(NumericFailure::DivisionByZero);
            }
            try_div(left, right)
        }
        NumericOperation::Modulo => {
            if rational_is_zero(right) {
                return Err(NumericFailure::DivisionByZero);
            }
            let quotient = try_div(left, right)?;
            let floored = rational_floor(&quotient)?;
            let product = try_mul(&floored, right)?;
            try_sub(left, &product)
        }
        NumericOperation::Power => try_rational_power(left, right),
    }
}

pub fn rational_operation_with_fallback(
    left: &RationalInteger,
    operation: NumericOperation,
    right: &RationalInteger,
) -> Result<RationalInteger, NumericFailure> {
    match rational_operation(left, operation, right) {
        Ok(result) => Ok(result),
        Err(NumericFailure::DivisionByZero) => Err(NumericFailure::DivisionByZero),
        Err(NumericFailure::Overflow) => Err(NumericFailure::Overflow),
        Err(NumericFailure::OutOfMemory) => Err(NumericFailure::OutOfMemory),
        Err(NumericFailure::MagnitudeLimit) => Err(NumericFailure::MagnitudeLimit),
        Err(NumericFailure::Irrational) => approximate_rational_operation(left, operation, right),
    }
}

fn approximate_rational_operation(
    left: &RationalInteger,
    operation: NumericOperation,
    right: &RationalInteger,
) -> Result<RationalInteger, NumericFailure> {
    let left_decimal = left.try_to_decimal()?;
    let right_decimal = right.try_to_decimal()?;
    let result_decimal = decimal_arithmetic(left_decimal, operation, right_decimal)?;
    decimal_to_rational(result_decimal)
}

fn decimal_arithmetic(
    left: Decimal,
    operation: NumericOperation,
    right: Decimal,
) -> Result<Decimal, NumericFailure> {
    match operation {
        NumericOperation::Add => left.checked_add(right).ok_or(NumericFailure::Overflow),
        NumericOperation::Subtract => left.checked_sub(right).ok_or(NumericFailure::Overflow),
        NumericOperation::Multiply => left.checked_mul(right).ok_or(NumericFailure::Overflow),
        NumericOperation::Divide => {
            if right.is_zero() {
                return Err(NumericFailure::DivisionByZero);
            }
            left.checked_div(right).ok_or(NumericFailure::Overflow)
        }
        NumericOperation::Modulo => {
            if right.is_zero() {
                return Err(NumericFailure::DivisionByZero);
            }
            let quotient = left.checked_div(right).ok_or(NumericFailure::Overflow)?;
            let floored = quotient.floor();
            let product = floored.checked_mul(right).ok_or(NumericFailure::Overflow)?;
            left.checked_sub(product).ok_or(NumericFailure::Overflow)
        }
        NumericOperation::Power => decimal_power(left, right),
    }
}

fn decimal_is_half(exponent: Decimal) -> bool {
    exponent
        .checked_mul(Decimal::TWO)
        .is_some_and(|doubled| doubled == Decimal::ONE)
}

fn decimal_power(base: Decimal, exponent: Decimal) -> Result<Decimal, NumericFailure> {
    if exponent.fract().is_zero() {
        let exponent_i64 =
            i64::try_from(exponent.trunc().mantissa()).map_err(|_| NumericFailure::Overflow)?;
        return base
            .checked_powi(exponent_i64)
            .ok_or(NumericFailure::Overflow);
    }
    if decimal_is_half(exponent) {
        return base.sqrt().ok_or(NumericFailure::Irrational);
    }
    Err(NumericFailure::Irrational)
}

// -- Core four + power ---

pub fn try_add(
    left: &RationalInteger,
    right: &RationalInteger,
) -> Result<RationalInteger, NumericFailure> {
    if let (
        Repr::Small {
            numer: ln,
            denom: ld,
        },
        Repr::Small {
            numer: rn,
            denom: rd,
        },
    ) = (&left.repr, &right.repr)
    {
        // ln/ld + rn/rd = (ln*rd + rn*ld) / (ld*rd)
        if let (Some(a), Some(b), Some(d)) = (
            ln.checked_mul(*rd),
            rn.checked_mul(*ld),
            ld.checked_mul(*rd),
        ) {
            if let Some(n) = a.checked_add(b) {
                return RationalInteger::from_small(n, d);
            }
        }
    }
    // BigInt fallback
    let (ln, ld) = left.as_big_pair();
    let (rn, rd) = right.as_big_pair();
    let numerator = ln.try_mul(&rd)?.try_add(&rn.try_mul(&ld)?)?;
    let denominator = ld.try_mul(&rd)?;
    try_rational_new(numerator, denominator)
}

pub fn try_sub(
    left: &RationalInteger,
    right: &RationalInteger,
) -> Result<RationalInteger, NumericFailure> {
    if let (
        Repr::Small {
            numer: ln,
            denom: ld,
        },
        Repr::Small {
            numer: rn,
            denom: rd,
        },
    ) = (&left.repr, &right.repr)
    {
        if let (Some(a), Some(b), Some(d)) = (
            ln.checked_mul(*rd),
            rn.checked_mul(*ld),
            ld.checked_mul(*rd),
        ) {
            if let Some(n) = a.checked_sub(b) {
                return RationalInteger::from_small(n, d);
            }
        }
    }
    let (ln, ld) = left.as_big_pair();
    let (rn, rd) = right.as_big_pair();
    let numerator = ln.try_mul(&rd)?.try_sub(&rn.try_mul(&ld)?)?;
    let denominator = ld.try_mul(&rd)?;
    try_rational_new(numerator, denominator)
}

pub fn try_mul(
    left: &RationalInteger,
    right: &RationalInteger,
) -> Result<RationalInteger, NumericFailure> {
    if let (
        Repr::Small {
            numer: ln,
            denom: ld,
        },
        Repr::Small {
            numer: rn,
            denom: rd,
        },
    ) = (&left.repr, &right.repr)
    {
        if let (Some(n), Some(d)) = (ln.checked_mul(*rn), ld.checked_mul(*rd)) {
            return RationalInteger::from_small(n, d);
        }
    }
    if left.numer_is_zero() || right.numer_is_zero() {
        return Ok(rational_zero());
    }
    let (ln, ld) = left.as_big_pair();
    let (rn, rd) = right.as_big_pair();
    cross_cancelled_product((&ln, &ld), (&rn, &rd))
}

/// `(ln / ld) * (rn / rd)` for reduced, non-zero operands with positive `ld` and `rd`.
/// Cancels `gcd(ln, rd)` and `gcd(rn, ld)` first, so the result is reduced by
/// construction and no gcd runs on the product.
fn cross_cancelled_product(
    (ln, ld): (&BigInt, &BigInt),
    (rn, rd): (&BigInt, &BigInt),
) -> Result<RationalInteger, NumericFailure> {
    let left_cross = ln.try_gcd(rd)?;
    let right_cross = rn.try_gcd(ld)?;
    let numerator = ln
        .try_div_trunc(&left_cross)?
        .try_mul(&rn.try_div_trunc(&right_cross)?)?;
    let denominator = ld
        .try_div_trunc(&right_cross)?
        .try_mul(&rd.try_div_trunc(&left_cross)?)?;
    Ok(RationalInteger::from_coprime(numerator, denominator))
}

pub fn try_div(
    left: &RationalInteger,
    right: &RationalInteger,
) -> Result<RationalInteger, NumericFailure> {
    if right.numer_is_zero() {
        return Err(NumericFailure::DivisionByZero);
    }
    if let (
        Repr::Small {
            numer: ln,
            denom: ld,
        },
        Repr::Small {
            numer: rn,
            denom: rd,
        },
    ) = (&left.repr, &right.repr)
    {
        if let (Some(n), Some(d)) = (ln.checked_mul(*rd), ld.checked_mul(*rn)) {
            return RationalInteger::from_small(n, d);
        }
    }
    if left.numer_is_zero() {
        return Ok(rational_zero());
    }
    let (ln, ld) = left.as_big_pair();
    let (rn, rd) = right.try_reciprocal_pair()?;
    cross_cancelled_product((&ln, &ld), (&rn, &rd))
}

/// Exact `exponent`-th power of `base` for any integer exponent.
pub fn try_pow_integer(
    base: &RationalInteger,
    exponent: &BigInt,
) -> Result<RationalInteger, NumericFailure> {
    if exponent.is_zero() {
        return Ok(rational_one());
    }
    if exponent.is_negative() {
        try_pow_magnitude(&base.try_reciprocal()?, exponent.magnitude())
    } else {
        try_pow_magnitude(base, exponent.magnitude())
    }
}

/// `gcd(a, b) = 1` implies `gcd(a^e, b^e) = 1`, so the power of a reduced base is
/// reduced and needs no gcd.
fn try_pow_magnitude(
    base: &RationalInteger,
    exponent: &BigUint,
) -> Result<RationalInteger, NumericFailure> {
    if let (Repr::Small { numer, denom }, Some(small_exponent)) = (&base.repr, exponent.to_u32()) {
        if let (Some(n), Some(d)) = (
            numer.checked_pow(small_exponent),
            denom.checked_pow(small_exponent),
        ) {
            return Ok(RationalInteger::from_reduced_small(n, d));
        }
    }
    let (numer, denom) = base.as_big_pair();
    Ok(RationalInteger::from_coprime(
        numer.try_pow(exponent)?,
        denom.try_pow(exponent)?,
    ))
}

pub fn try_rational_power(
    base: &RationalInteger,
    exponent: &RationalInteger,
) -> Result<RationalInteger, NumericFailure> {
    if !exponent.numer_is_positive() && !exponent.numer_is_negative() && exponent.numer_is_zero() {
        return Ok(rational_one());
    }

    if exponent.is_integer() {
        let integer_exponent = exponent.to_big_numer();
        if power_exceeds_magnitude_limit(base, integer_exponent.magnitude()) {
            return Err(NumericFailure::MagnitudeLimit);
        }
        return try_pow_integer(base, &integer_exponent);
    }

    if base.numer_is_zero() {
        if exponent.numer_is_negative() || exponent.numer_is_zero() {
            return Err(NumericFailure::DivisionByZero);
        }
        return Ok(rational_new(0, 1));
    }

    let root_degree = exponent.to_big_denom().magnitude().clone();
    let abs_exponent = exponent.to_big_numer();

    let result_negative = base.numer_is_negative() && abs_exponent.magnitude().is_odd();
    if result_negative && !root_degree.is_odd() {
        return Err(NumericFailure::Irrational);
    }

    let numer_root = exact_integer_root(&base.to_big_numer().try_abs()?, &root_degree)?;
    let denom_root = exact_integer_root(&base.to_big_denom(), &root_degree)?;
    let signed_numer_root = if result_negative {
        numer_root.try_neg()?
    } else {
        numer_root
    };
    let root = RationalInteger::from_coprime(signed_numer_root, denom_root);
    if power_exceeds_magnitude_limit(&root, abs_exponent.magnitude()) {
        return Err(NumericFailure::MagnitudeLimit);
    }
    let raised = try_pow_magnitude(&root, abs_exponent.magnitude())?;

    if exponent.numer_is_negative() {
        raised.try_reciprocal()
    } else {
        Ok(raised)
    }
}

/// True when `base^exponent` provably has a numerator or denominator above
/// [`MAX_RATIONAL_BITS`]: a `b`-bit magnitude with `b >= 2` raised to `e` has at least
/// `(b - 1) * e + 1` bits. When false the result has at most twice the limit, so
/// computing it before the final check stays bounded.
fn power_exceeds_magnitude_limit(base: &RationalInteger, exponent: &BigUint) -> bool {
    [base.numer_magnitude_bits(), base.denom_magnitude_bits()]
        .into_iter()
        .filter(|bits| *bits >= 2)
        .any(|bits| match exponent.to_u32() {
            Some(exponent) => (bits - 1)
                .checked_mul(u64::from(exponent))
                .is_none_or(|lower_bound| lower_bound >= MAX_RATIONAL_BITS),
            None => true,
        })
}

/// Exact integer `degree`-th root of a positive `value`; `Irrational` when none exists.
fn exact_integer_root(value: &BigInt, degree: &BigUint) -> Result<BigInt, NumericFailure> {
    let root = value.try_nth_root(degree)?;
    if root.try_pow(degree)? == *value {
        Ok(root)
    } else {
        Err(NumericFailure::Irrational)
    }
}

pub use {try_div as checked_div, try_mul as checked_mul, try_pow_integer as checked_pow_integer};

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal::Decimal;
    use std::str::FromStr;

    #[test]
    fn power_of_two_with_all_ones_exponent_is_positive() {
        let two_pow_127 =
            RationalInteger::from_bigint_strings("170141183460469231731687303715884105728", "1")
                .unwrap();
        assert!(two_pow_127.numer_is_positive(), "parsed 2^127 sign");
        let big =
            BigInt::try_from_str_radix("170141183460469231731687303715884105728", 10).unwrap();
        assert!(big.is_positive(), "BigInt 2^127 sign");
        let product = try_mul(&rational_new(1 << 62, 1), &rational_new(1 << 62, 1)).unwrap();
        let product = try_mul(&product, &rational_new(8, 1)).unwrap();
        assert!(product.numer_is_positive(), "2^62 * 2^62 * 8 sign");
        let powered = try_pow_integer(&rational_new(2, 1), &BigInt::from_i64(127)).unwrap();
        assert!(powered.numer_is_positive(), "2^127 via power sign");
        assert_eq!(powered, two_pow_127);
    }

    #[test]
    fn i128_min_numerator_promotes_instead_of_failing() {
        let two_pow_127 =
            RationalInteger::from_bigint_strings("170141183460469231731687303715884105728", "1")
                .unwrap();
        let min = RationalInteger::from_small(i128::MIN, 1).unwrap();
        assert_eq!(rational_abs(&min).unwrap(), two_pow_127);
        assert_eq!(try_div(&min, &rational_new(-1, 1)).unwrap(), two_pow_127);
        let reciprocal = try_div(&rational_new(1, 1), &min).unwrap();
        assert_eq!(
            reciprocal,
            RationalInteger::from_bigint_strings("-1", "170141183460469231731687303715884105728")
                .unwrap()
        );
        assert_eq!(
            RationalInteger::from_small(i128::MIN, -2).unwrap(),
            RationalInteger::from_bigint_strings("85070591730234615865843651857942052864", "1")
                .unwrap()
        );
        assert_eq!(
            RationalInteger::from_small(1, i128::MIN).unwrap(),
            RationalInteger::from_bigint_strings("-1", "170141183460469231731687303715884105728")
                .unwrap()
        );
        assert_eq!(
            RationalInteger::from_small(i128::MIN, i128::MIN).unwrap(),
            rational_one()
        );
        assert_eq!(
            RationalInteger::from_small(3, i128::MIN).unwrap(),
            RationalInteger::from_bigint_strings("-3", "170141183460469231731687303715884105728")
                .unwrap()
        );
    }

    fn two_pow(exponent: i64) -> RationalInteger {
        try_pow_integer(&rational_new(2, 1), &BigInt::from_i64(exponent)).unwrap()
    }

    fn operate(
        left: &RationalInteger,
        operation: NumericOperation,
        right: &RationalInteger,
    ) -> Result<RationalInteger, NumericFailure> {
        rational_operation(left, operation, right)
    }

    fn negate(value: &RationalInteger) -> RationalInteger {
        try_sub(&rational_zero(), value).unwrap()
    }

    const LIMIT: i64 = MAX_RATIONAL_BITS as i64;

    #[test]
    fn rational_operation_accepts_results_at_the_bit_limit() {
        let at_limit = two_pow(LIMIT - 1);
        assert_eq!(at_limit.numer_magnitude_bits(), MAX_RATIONAL_BITS);
        let half_limit = two_pow(LIMIT / 2 - 1);
        assert_eq!(
            operate(&half_limit, NumericOperation::Multiply, &two_pow(LIMIT / 2)).unwrap(),
            at_limit
        );
        assert_eq!(
            operate(
                &two_pow(LIMIT - 2),
                NumericOperation::Add,
                &two_pow(LIMIT - 2)
            )
            .unwrap(),
            at_limit
        );
        let reciprocal = operate(&rational_one(), NumericOperation::Divide, &at_limit).unwrap();
        assert_eq!(reciprocal.denom_magnitude_bits(), MAX_RATIONAL_BITS);
    }

    #[test]
    fn rational_operation_rejects_numerator_beyond_the_bit_limit() {
        let at_limit = two_pow(LIMIT - 1);
        let two = rational_new(2, 1);
        for (operation, right) in [
            (NumericOperation::Add, at_limit.clone()),
            (NumericOperation::Subtract, negate(&at_limit)),
            (NumericOperation::Multiply, two.clone()),
            (NumericOperation::Divide, rational_new(1, 2)),
            (NumericOperation::Power, two.clone()),
        ] {
            assert_eq!(
                operate(&at_limit, operation, &right).unwrap_err(),
                NumericFailure::MagnitudeLimit,
                "{operation:?}"
            );
        }
    }

    #[test]
    fn rational_operation_rejects_denominator_beyond_the_bit_limit() {
        let reciprocal_at_limit = try_div(&rational_one(), &two_pow(LIMIT - 1)).unwrap();
        assert_eq!(
            operate(
                &reciprocal_at_limit,
                NumericOperation::Divide,
                &rational_new(2, 1)
            )
            .unwrap_err(),
            NumericFailure::MagnitudeLimit
        );
        assert_eq!(
            operate(
                &reciprocal_at_limit,
                NumericOperation::Multiply,
                &rational_new(1, 2)
            )
            .unwrap_err(),
            NumericFailure::MagnitudeLimit
        );
    }

    #[test]
    fn power_precheck_is_exact_at_the_bit_limit() {
        let two = rational_new(2, 1);
        assert_eq!(
            operate(&two, NumericOperation::Power, &rational_new(LIMIT - 1, 1)).unwrap(),
            two_pow(LIMIT - 1)
        );
        assert!(!power_exceeds_magnitude_limit(
            &two,
            &BigUint::try_from_u32(LIMIT as u32 - 1).unwrap()
        ));
        assert!(power_exceeds_magnitude_limit(
            &two,
            &BigUint::try_from_u32(LIMIT as u32).unwrap()
        ));
        let two_pow_sixteen = rational_new(1 << 16, 1);
        let largest_fitting_exponent = (LIMIT as u32 - 1) / 16;
        assert!(!power_exceeds_magnitude_limit(
            &two_pow_sixteen,
            &BigUint::try_from_u32(largest_fitting_exponent).unwrap()
        ));
        assert_eq!(
            operate(
                &two_pow_sixteen,
                NumericOperation::Power,
                &rational_new(i64::from(largest_fitting_exponent), 1)
            )
            .unwrap(),
            two_pow(16 * i64::from(largest_fitting_exponent))
        );
    }

    #[test]
    fn power_precheck_rejects_huge_exponents_before_allocating() {
        let huge = RationalInteger::from_bigint_strings("1180591620717411303424", "1").unwrap();
        for base in [rational_new(2, 1), rational_new(1, 2), rational_new(-3, 7)] {
            assert_eq!(
                operate(&base, NumericOperation::Power, &huge).unwrap_err(),
                NumericFailure::MagnitudeLimit,
                "{base:?}"
            );
            assert_eq!(
                operate(&base, NumericOperation::Power, &negate(&huge)).unwrap_err(),
                NumericFailure::MagnitudeLimit,
                "{base:?} negative exponent"
            );
        }
        assert_eq!(
            operate(&rational_new(-1, 1), NumericOperation::Power, &huge).unwrap(),
            rational_one()
        );
    }

    #[test]
    fn power_precheck_applies_to_the_root_of_fractional_exponents() {
        let four = rational_new(4, 1);
        assert_eq!(
            operate(&four, NumericOperation::Power, &rational_new(LIMIT - 1, 2)).unwrap(),
            two_pow(LIMIT - 1)
        );
        assert_eq!(
            operate(&four, NumericOperation::Power, &rational_new(LIMIT + 1, 2)).unwrap_err(),
            NumericFailure::MagnitudeLimit
        );
    }

    use crate::computation::bigint::work_counters;

    fn big(value: i64) -> BigInt {
        BigInt::from_i64(value)
    }

    fn big_pow(base: i64, exponent: u32) -> BigInt {
        big(base)
            .try_pow(&BigUint::try_from_u32(exponent).unwrap())
            .unwrap()
    }

    fn big_add(left: &BigInt, right: &BigInt) -> BigInt {
        left.try_add(right).unwrap()
    }

    fn fraction(numer: BigInt, denom: BigInt) -> RationalInteger {
        try_rational_new(numer, denom).unwrap()
    }

    fn assert_canonical(value: &RationalInteger) {
        if let Err(reason) = RationalInteger::from_canonical_repr(value.repr.clone()) {
            panic!("{value:?} is not canonical: {reason}");
        }
    }

    fn reference_mul(left: &RationalInteger, right: &RationalInteger) -> RationalInteger {
        let (ln, ld) = left.as_big_pair();
        let (rn, rd) = right.as_big_pair();
        fraction(ln.try_mul(&rn).unwrap(), ld.try_mul(&rd).unwrap())
    }

    fn reference_div(left: &RationalInteger, right: &RationalInteger) -> RationalInteger {
        let (ln, ld) = left.as_big_pair();
        let (rn, rd) = right.as_big_pair();
        fraction(ln.try_mul(&rd).unwrap(), ld.try_mul(&rn).unwrap())
    }

    fn reference_pow(base: &RationalInteger, exponent: i64) -> RationalInteger {
        let (n, d) = base.as_big_pair();
        let magnitude = BigUint::try_from_u64(exponent.unsigned_abs()).unwrap();
        let (n, d) = (
            n.try_pow(&magnitude).unwrap(),
            d.try_pow(&magnitude).unwrap(),
        );
        if exponent < 0 {
            fraction(d, n)
        } else {
            fraction(n, d)
        }
    }

    /// `2^32767 - 1` and `2^32767 + 1`: odd, two apart, hence coprime; 32767 and 32768 bits.
    fn cap_sized_coprime_pair() -> (BigInt, BigInt) {
        let power = big_pow(2, 32_767);
        (big_add(&power, &big(-1)), big_add(&power, &big(1)))
    }

    fn arithmetic_fixtures() -> Vec<RationalInteger> {
        vec![
            rational_zero(),
            rational_one(),
            rational_new(-1, 1),
            rational_new(3, 7),
            rational_new(-22, 15),
            RationalInteger::from_small(i128::MAX, 1).unwrap(),
            RationalInteger::from_small(i128::MIN, 1).unwrap(),
            RationalInteger::from_small(1, i128::MAX).unwrap(),
            RationalInteger::from_small(i128::MIN, i128::MAX).unwrap(),
            fraction(big_add(&big_pow(2, 200), &big(1)), big_pow(3, 100)),
            fraction(big_pow(-5, 91), big_pow(2, 150)),
            fraction(big_pow(6, 80), big_pow(35, 40)),
            fraction(big_pow(35, 40), big_pow(-6, 81)),
            fraction(big_pow(2, 130), big(3)),
        ]
    }

    #[test]
    fn mul_div_pow_match_reduction_of_the_raw_result() {
        let fixtures = arithmetic_fixtures();
        for left in &fixtures {
            for right in &fixtures {
                let product = try_mul(left, right).unwrap();
                assert_canonical(&product);
                assert_eq!(product, reference_mul(left, right), "{left:?} * {right:?}");
                if right.numer_is_zero() {
                    continue;
                }
                let quotient = try_div(left, right).unwrap();
                assert_canonical(&quotient);
                assert_eq!(quotient, reference_div(left, right), "{left:?} / {right:?}");
            }
            for exponent in -3..=3 {
                if exponent < 0 && left.numer_is_zero() {
                    continue;
                }
                let power = try_pow_integer(left, &BigInt::from_i64(exponent)).unwrap();
                assert_canonical(&power);
                assert_eq!(
                    power,
                    reference_pow(left, exponent),
                    "{left:?} ^ {exponent}"
                );
            }
        }
    }

    #[test]
    fn try_div_keeps_denominator_positive_for_every_sign() {
        let (a, b) = cap_sized_coprime_pair();
        let positive = fraction(a.try_clone().unwrap(), big_pow(3, 50));
        let other = fraction(big_pow(7, 60), b);
        let negate = |value: &RationalInteger| try_sub(&rational_zero(), value).unwrap();
        for (left, right) in [
            (positive.clone(), negate(&other)),
            (negate(&positive), other.clone()),
            (negate(&positive), negate(&other)),
            (positive.clone(), other.clone()),
        ] {
            let quotient = try_div(&left, &right).unwrap();
            assert_canonical(&quotient);
            assert_eq!(quotient, reference_div(&left, &right));
        }
        assert_eq!(
            try_div(&positive, &negate(&positive)).unwrap(),
            rational_new(-1, 1)
        );
    }

    #[test]
    fn power_of_a_reduced_fraction_performs_no_gcd() {
        let bases = [
            rational_new(7301, 7300),
            rational_new(-3, 7),
            fraction(big_add(&big_pow(2, 200), &big(1)), big_pow(3, 130)),
            fraction(big_pow(-5, 91), big_pow(2, 150)),
        ];
        for base in &bases {
            for exponent in [-301, -2, 2, 5, 200] {
                work_counters::reset();
                let power = try_pow_integer(base, &BigInt::from_i64(exponent)).unwrap();
                let counts = work_counters::snapshot();
                assert_eq!(counts.gcd_calls, 0, "{base:?} ^ {exponent}");
                assert_canonical(&power);
                assert_eq!(
                    power,
                    reference_pow(base, exponent),
                    "{base:?} ^ {exponent}"
                );
            }
        }
    }

    #[test]
    fn product_of_reciprocals_takes_gcds_of_the_operands_only() {
        let (a, b) = cap_sized_coprime_pair();
        let operand_bits = a.bits().max(b.bits());
        let left = fraction(a.try_clone().unwrap(), b.try_clone().unwrap());
        let right = fraction(b, a);
        work_counters::reset();
        assert_eq!(try_mul(&left, &right).unwrap(), rational_one());
        let counts = work_counters::snapshot();
        assert_eq!(counts.gcd_calls, 2);
        assert!(counts.gcd_max_operand_bits <= operand_bits);
        work_counters::reset();
        assert_eq!(try_div(&left, &left).unwrap(), rational_one());
        let counts = work_counters::snapshot();
        assert_eq!(counts.gcd_calls, 2);
        assert!(counts.gcd_max_operand_bits <= operand_bits);
    }

    #[test]
    fn rational_zero_is_zero() {
        assert!(rational_is_zero(&rational_zero()));
    }

    #[test]
    fn decimal_one_half_lifts_to_rational() {
        let decimal = Decimal::from_str("0.5").unwrap();
        let rational = decimal_to_rational(decimal).unwrap();
        assert_eq!(rational, rational_new(1, 2));
    }

    #[test]
    fn try_to_decimal_one_third() {
        let rational = rational_new(1, 3);
        let decimal = rational.try_to_decimal().unwrap();
        let expected = Decimal::from_str("0.3333333333333333333333333333").unwrap();
        assert_eq!(decimal, expected);
    }

    #[test]
    fn try_to_decimal_tiny_rounds_to_zero() {
        let rational =
            RationalInteger::from_bigint_strings("1", "1000000000000000000000000000000").unwrap();
        assert_eq!(rational.try_to_decimal().unwrap(), Decimal::ZERO);
    }

    #[test]
    fn try_to_decimal_magnitude_beyond_max_returns_overflow() {
        let max = Decimal::MAX.normalize();
        let max_rational = decimal_to_rational(max).unwrap();
        let twice = try_mul(&max_rational, &rational_new(2, 1)).unwrap();
        assert_eq!(
            twice.try_to_decimal().unwrap_err(),
            NumericFailure::Overflow,
        );
    }

    #[test]
    fn try_to_decimal_string_integer() {
        let rational = rational_new(37, 1);
        assert_eq!(rational.try_to_decimal_string().unwrap(), "37");
    }

    #[test]
    fn display_str_shows_decimal_when_try_to_decimal_succeeds() {
        let rational = rational_new(355, 113);
        let display = rational.display_str();
        assert!(
            !display.contains('/'),
            "rational with successful try_to_decimal must display as decimal, got {display}"
        );
    }

    #[test]
    fn try_to_decimal_huge_cancelling_rationals() {
        let numer = BigInt::try_from_str_radix("1", 10)
            .unwrap()
            .try_pow(&BigUint::try_from_u32(100).unwrap())
            .unwrap();
        let rational = try_rational_new(numer.clone(), numer).unwrap();
        assert_eq!(rational.try_to_decimal().unwrap(), Decimal::ONE);
    }

    #[test]
    fn try_mul_integer() {
        let left = rational_new(50, 1);
        let right = rational_new(86400, 1);
        let product = try_mul(&left, &right).unwrap();
        assert_eq!(product, rational_new(4_320_000, 1));
    }

    #[test]
    fn try_pow_negative_exponent_inverts_base() {
        let hour_factor = rational_new(3600, 1);
        let inverse = try_pow_integer(&hour_factor, &BigInt::from_i64(-1)).unwrap();
        assert_eq!(inverse, rational_new(1, 3600));
    }

    #[test]
    fn rational_operation_divide_by_zero() {
        let left = rational_new(1, 1);
        let right = rational_new(0, 1);
        let failure = rational_operation(&left, NumericOperation::Divide, &right).unwrap_err();
        assert_eq!(failure, NumericFailure::DivisionByZero);
    }

    #[test]
    fn rational_operation_power_irrational() {
        let base = rational_new(2, 1);
        let exponent = rational_new(1, 2);
        let failure = rational_operation(&base, NumericOperation::Power, &exponent).unwrap_err();
        assert_eq!(failure, NumericFailure::Irrational);
    }

    #[test]
    fn rational_operation_power_exact() {
        let base = rational_new(4, 1);
        let exponent = rational_new(1, 2);
        let result = rational_operation(&base, NumericOperation::Power, &exponent).unwrap();
        assert_eq!(result, rational_new(2, 1));
    }

    #[test]
    fn rational_power_exact_roots_of_rationals() {
        let power = |base: RationalInteger, exponent: RationalInteger| {
            rational_operation(&base, NumericOperation::Power, &exponent)
        };
        assert_eq!(
            power(rational_new(9, 4), rational_new(1, 2)).unwrap(),
            rational_new(3, 2)
        );
        assert_eq!(
            power(rational_new(9, 4), rational_new(-1, 2)).unwrap(),
            rational_new(2, 3)
        );
        assert_eq!(
            power(rational_new(8, 1), rational_new(2, 3)).unwrap(),
            rational_new(4, 1)
        );
        assert_eq!(
            power(rational_new(-8, 1), rational_new(1, 3)).unwrap(),
            rational_new(-2, 1)
        );
        assert_eq!(
            power(rational_new(-4, 1), rational_new(2, 1)).unwrap(),
            rational_new(16, 1)
        );
        assert_eq!(
            power(rational_new(1, 1), rational_new(49_595, 100_000)).unwrap(),
            rational_new(1, 1)
        );
    }

    #[test]
    fn power_with_huge_integer_exponent_of_one_and_minus_one() {
        let even_huge =
            RationalInteger::from_bigint_strings("1000000000000000000000000000000", "1").unwrap();
        let odd_huge =
            RationalInteger::from_bigint_strings("1000000000000000000000000000001", "1").unwrap();
        assert_eq!(
            rational_operation(&rational_new(1, 1), NumericOperation::Power, &even_huge).unwrap(),
            rational_new(1, 1)
        );
        assert_eq!(
            rational_operation(&rational_new(-1, 1), NumericOperation::Power, &odd_huge).unwrap(),
            rational_new(-1, 1)
        );
    }

    #[test]
    fn power_with_huge_root_degree_above_bit_length_is_irrational() {
        let degree = RationalInteger::from_bigint_strings("1", "1000000000000").unwrap();
        assert_eq!(
            rational_operation(&rational_new(2, 1), NumericOperation::Power, &degree).unwrap_err(),
            NumericFailure::Irrational
        );
    }

    #[test]
    fn power_with_exponent_beyond_every_allocation_is_out_of_memory() {
        let exponent = BigInt::from_i128(1i128 << 70);
        assert_eq!(
            try_pow_integer(&rational_new(2, 1), &exponent).unwrap_err(),
            NumericFailure::OutOfMemory
        );
        assert_eq!(
            try_pow_integer(&rational_new(1, 1), &exponent).unwrap(),
            rational_new(1, 1)
        );
    }

    #[test]
    fn rational_power_irrational_fails_fast_for_large_degree() {
        let failure = rational_operation(
            &rational_new(480, 1),
            NumericOperation::Power,
            &rational_new(49_595, 100_000),
        )
        .unwrap_err();
        assert_eq!(failure, NumericFailure::Irrational);
        let failure = rational_operation(
            &rational_new(2, 1),
            NumericOperation::Power,
            &rational_new(49_595, 100_000),
        )
        .unwrap_err();
        assert_eq!(failure, NumericFailure::Irrational);
    }

    #[test]
    fn rational_power_negative_base_even_root_is_irrational() {
        let failure = rational_operation(
            &rational_new(-4, 1),
            NumericOperation::Power,
            &rational_new(1, 2),
        )
        .unwrap_err();
        assert_eq!(failure, NumericFailure::Irrational);
    }

    #[test]
    fn rational_operation_add() {
        let left = rational_new(1, 3);
        let right = rational_new(1, 6);
        let sum = rational_operation(&left, NumericOperation::Add, &right).unwrap();
        assert_eq!(sum, rational_new(1, 2));
    }

    #[test]
    fn rational_operation_with_fallback_add_exact_rational() {
        let left = rational_new(1, 3);
        let right = rational_new(1, 6);
        let sum = rational_operation_with_fallback(&left, NumericOperation::Add, &right).unwrap();
        assert_eq!(sum, rational_new(1, 2));
    }

    #[test]
    fn rational_operation_with_fallback_power_sqrt_via_decimal() {
        let result = rational_operation_with_fallback(
            &rational_new(2, 1),
            NumericOperation::Power,
            &rational_new(1, 2),
        )
        .unwrap();
        let expected = rational_new(2, 1).try_to_decimal().unwrap().sqrt().unwrap();
        assert_eq!(
            result.try_to_decimal().unwrap().round_dp(27),
            expected.round_dp(27),
        );
    }

    #[test]
    fn rational_modulo_is_floored_and_takes_divisor_sign() {
        assert_eq!(
            operate(
                &rational_new(-7, 1),
                NumericOperation::Modulo,
                &rational_new(3, 1)
            )
            .unwrap(),
            rational_new(2, 1)
        );
        assert_eq!(
            operate(
                &rational_new(7, 1),
                NumericOperation::Modulo,
                &rational_new(-3, 1)
            )
            .unwrap(),
            rational_new(-2, 1)
        );
        assert_eq!(
            operate(
                &rational_new(-7, 2),
                NumericOperation::Modulo,
                &rational_new(1, 1)
            )
            .unwrap(),
            rational_new(1, 2)
        );
        assert_eq!(
            operate(
                &rational_new(-7, 1),
                NumericOperation::Modulo,
                &rational_new(-3, 1)
            )
            .unwrap(),
            rational_new(-1, 1)
        );
    }

    #[test]
    fn rational_abs_negates_negative_numerator() {
        let negative = rational_new(-172_800, 1);
        assert_eq!(rational_abs(&negative).unwrap(), rational_new(172_800, 1));
    }

    #[test]
    fn try_to_decimal_string_rejects_magnitude_overflow() {
        let too_large =
            RationalInteger::from_bigint_strings("10000000000000000000000000000000", "1").unwrap();
        assert_eq!(
            too_large.try_to_decimal().unwrap_err(),
            NumericFailure::Overflow
        );
        assert_eq!(
            too_large.try_to_decimal_string().unwrap_err(),
            NumericFailure::Overflow,
        );
    }

    #[test]
    fn decimal_max_times_decimal_max_stays_exact_without_decimal_fallback() {
        let max_decimal = Decimal::MAX.normalize();
        let left = decimal_to_rational(max_decimal).unwrap();
        let right = decimal_to_rational(max_decimal).unwrap();
        let product = rational_operation(&left, NumericOperation::Multiply, &right).unwrap();
        let expected = try_mul(&left, &right).unwrap();
        assert_eq!(product, expected);
    }

    #[test]
    fn alloc_error_maps_to_out_of_memory() {
        assert_eq!(
            NumericFailure::from(crate::computation::bigint::AllocError),
            NumericFailure::OutOfMemory
        );
    }

    #[test]
    fn small_path_basic_arithmetic() {
        let a = rational_new(3, 1);
        let b = rational_new(5, 1);
        assert_eq!(try_add(&a, &b).unwrap(), rational_new(8, 1));
        assert_eq!(try_sub(&a, &b).unwrap(), rational_new(-2, 1));
        assert_eq!(try_mul(&a, &b).unwrap(), rational_new(15, 1));
        assert_eq!(try_div(&a, &b).unwrap(), rational_new(3, 5));
    }

    #[test]
    fn small_path_reduces() {
        let r = rational_new(6, 4);
        assert_eq!(r, rational_new(3, 2));
    }

    #[test]
    fn small_path_try_to_decimal_integer() {
        let r = rational_new(42, 1);
        assert_eq!(r.try_to_decimal().unwrap(), Decimal::from(42));
    }

    #[test]
    fn serde_round_trip_one_third() {
        let value = rational_new(1, 3);
        let bytes = serde_json::to_vec(&value).expect("serialize");
        let restored: RationalInteger = serde_json::from_slice(&bytes).expect("deserialize");
        assert_eq!(restored, value);
    }

    #[test]
    fn serde_round_trip_negative_and_huge() {
        let value = RationalInteger::from_bigint_strings(
            "-123456789012345678901234567890",
            "987654321098765432109876543210",
        )
        .unwrap();
        let bytes = serde_json::to_vec(&value).expect("serialize");
        let restored: RationalInteger = serde_json::from_slice(&bytes).expect("deserialize");
        assert_eq!(restored, value);
    }

    #[test]
    fn serde_round_trip_beyond_i128() {
        let value = RationalInteger::from_bigint_strings(
            "-1234567890123456789012345678901234567890123",
            "9876543210987654321098765432109876543210987",
        )
        .unwrap();
        assert!(
            matches!(value.repr, Repr::Big { .. }),
            "fixture must exercise the Big variant"
        );
        let bytes = postcard::to_allocvec(&value).expect("serialize");
        let restored: RationalInteger = postcard::from_bytes(&bytes).expect("deserialize");
        assert_eq!(restored, value);
    }

    #[test]
    fn serde_small_is_two_varints() {
        let bytes = postcard::to_allocvec(&rational_new(1, 3)).expect("serialize");
        assert_eq!(bytes, [0x00, 0x02, 0x06]);
    }

    fn reject(repr: Repr, expected: &str) {
        let payload = serde_json::to_vec(&repr).unwrap();
        match serde_json::from_slice::<RationalInteger>(&payload) {
            Ok(_) => panic!("{repr:?} must fail"),
            Err(err) => assert!(
                err.to_string().contains(expected),
                "unexpected error for {repr:?}: {err}"
            ),
        }
    }

    #[test]
    fn serde_rejects_zero_denominator() {
        reject(Repr::Small { numer: 1, denom: 0 }, "must be positive");
        reject(
            Repr::Big {
                numer: BigInt::one(),
                denom: BigInt::zero(),
            },
            "must be positive",
        );
    }

    #[test]
    fn serde_rejects_negative_denominator() {
        reject(
            Repr::Small {
                numer: 1,
                denom: -3,
            },
            "must be positive",
        );
    }

    #[test]
    fn serde_rejects_unreduced_small() {
        reject(Repr::Small { numer: 2, denom: 4 }, "not reduced");
        reject(Repr::Small { numer: 0, denom: 5 }, "not reduced");
    }

    #[test]
    fn serde_rejects_big_that_fits_small() {
        reject(
            Repr::Big {
                numer: BigInt::one(),
                denom: BigInt::from_i128(3),
            },
            "must be encoded Small",
        );
    }

    #[test]
    fn serde_rejects_unreduced_big() {
        let huge =
            BigInt::try_from_str_radix("9876543210987654321098765432109876543210987", 10).unwrap();
        let two = BigInt::from_i128(2);
        let numer = huge.try_mul(&two).unwrap();
        let denom = huge.try_mul(&two).unwrap();
        reject(Repr::Big { numer, denom }, "not reduced");
    }
}
