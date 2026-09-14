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
pub(crate) use crate::computation::bigint::BigInt;

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
    let g = gcd_u128(numer.unsigned_abs(), denom.unsigned_abs()) as i128;
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
            // numer.checked_neg / denom.checked_neg can fail for i128::MIN
            let n = numer.checked_neg().ok_or(NumericFailure::OutOfMemory)?;
            let d = denom.checked_neg().ok_or(NumericFailure::OutOfMemory)?;
            (n, d)
        } else {
            (numer, denom)
        };
        let (rn, rd) = reduce_small(n, d);
        Ok(Self::from_reduced_small(rn, rd))
    }

    pub fn try_new(numer: BigInt, denom: BigInt) -> Result<Self, NumericFailure> {
        if denom.is_zero() {
            return Err(NumericFailure::DivisionByZero);
        }
        // Try small path first
        if let (Some(n), Some(d)) = (numer.to_i128(), denom.to_i128()) {
            return Self::from_small(n, d);
        }
        // Big path
        let value = Self {
            repr: Repr::Big { numer, denom },
        };
        value.try_reduce()
    }

    pub fn from_i64_pair(numer: i64, denom: i64) -> Self {
        Self::from_small(i128::from(numer), i128::from(denom))
            .expect("BUG: i64 rational reduce cannot fail")
    }

    /// Construct from string-parsed BigInts (used by `SerializedFactor` round-trip).
    pub fn from_bigint_strings(numer_str: &str, denom_str: &str) -> Result<Self, NumericFailure> {
        let n = BigInt::try_from_str_radix(numer_str, 10).map_err(map_alloc)?;
        let d = BigInt::try_from_str_radix(denom_str, 10).map_err(map_alloc)?;
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

    pub fn denom_to_u32(&self) -> Option<u32> {
        match &self.repr {
            Repr::Small { denom, .. } => u32::try_from(*denom).ok(),
            Repr::Big { denom, .. } => denom.to_u32(),
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

    /// Approximate bit count of the numerator's magnitude (for structural size estimates).
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

    /// Approximate bit count of the denominator's magnitude (for structural size estimates).
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
            Repr::Big { numer, denom } => {
                if denom.is_zero() {
                    return Err(NumericFailure::DivisionByZero);
                }
                let mut n = numer.clone();
                let mut d = denom.clone();
                if d.is_negative() {
                    n = n.try_neg().map_err(map_alloc)?;
                    d = d.try_neg().map_err(map_alloc)?;
                }
                if n.is_zero() {
                    return Ok(Self::from_reduced_small(0, 1));
                }
                let gcd = n.try_abs()?.try_gcd(&d.try_abs()?)?;
                n = n.try_div_trunc(&gcd)?;
                d = d.try_div_trunc(&gcd)?;
                // Try to shrink back to small
                if let (Some(sn), Some(sd)) = (n.to_i128(), d.to_i128()) {
                    Ok(Self::from_reduced_small(sn, sd))
                } else {
                    Ok(Self {
                        repr: Repr::Big { numer: n, denom: d },
                    })
                }
            }
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
    Irrational,
}

impl fmt::Display for NumericFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            NumericFailure::DivisionByZero => formatter.write_str("division by zero"),
            NumericFailure::Overflow => formatter.write_str("numeric overflow"),
            NumericFailure::OutOfMemory => formatter.write_str("out of memory"),
            NumericFailure::Irrational => formatter.write_str("irrational numeric result"),
        }
    }
}

fn map_alloc(_: AllocError) -> NumericFailure {
    NumericFailure::OutOfMemory
}

impl From<AllocError> for NumericFailure {
    fn from(err: AllocError) -> Self {
        map_alloc(err)
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
        Repr::Small { numer, denom } => {
            let n = numer.checked_abs().ok_or(NumericFailure::OutOfMemory)?;
            Ok(RationalInteger::from_reduced_small(n, *denom))
        }
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
            .map_err(map_alloc)?;
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

pub fn rational_operation(
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
            let truncated = rational_trunc(&quotient)?;
            let product = try_mul(&truncated, right)?;
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
            let truncated = quotient.trunc();
            let product = truncated
                .checked_mul(right)
                .ok_or(NumericFailure::Overflow)?;
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
    let (ln, ld) = left.as_big_pair();
    let (rn, rd) = right.as_big_pair();
    let numerator = ln.try_mul(&rn)?;
    let denominator = ld.try_mul(&rd)?;
    try_rational_new(numerator, denominator)
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
    let (ln, ld) = left.as_big_pair();
    let (rn, rd) = right.as_big_pair();
    let numerator = ln.try_mul(&rd)?;
    let denominator = ld.try_mul(&rn)?;
    try_rational_new(numerator, denominator)
}

pub fn try_pow_i32(
    base: &RationalInteger,
    exponent: i32,
) -> Result<RationalInteger, NumericFailure> {
    if exponent == 0 {
        return Ok(rational_one());
    }
    if exponent < 0 {
        if base.numer_is_zero() {
            return Err(NumericFailure::DivisionByZero);
        }
        // Invert and recurse with positive exponent
        let inverted = match &base.repr {
            Repr::Small { numer, denom } => RationalInteger::from_small(*denom, *numer)?,
            Repr::Big { numer, denom } => try_rational_new(denom.try_clone()?, numer.try_clone()?)?,
        };
        return try_pow_i32(&inverted, -exponent);
    }
    let mut result = rational_one();
    let mut factor = base.clone();
    let mut remaining = exponent as u32;
    while remaining > 0 {
        if remaining % 2 == 1 {
            result = try_mul(&result, &factor)?;
        }
        remaining /= 2;
        if remaining > 0 {
            factor = try_mul(&factor, &factor)?;
        }
    }
    Ok(result)
}

pub fn try_rational_power(
    base: &RationalInteger,
    exponent: &RationalInteger,
) -> Result<RationalInteger, NumericFailure> {
    if !exponent.numer_is_positive() && !exponent.numer_is_negative() && exponent.numer_is_zero() {
        return Ok(rational_one());
    }

    let exp_denom_u32 = exponent.denom_to_u32();
    let exp_is_integer = exponent.is_integer();

    if exp_is_integer {
        let exponent_i32 = exponent.numer_to_i32().ok_or(NumericFailure::Overflow)?;
        return try_pow_i32(base, exponent_i32);
    }

    if base.numer_is_zero() {
        if exponent.numer_is_negative() || exponent.numer_is_zero() {
            return Err(NumericFailure::DivisionByZero);
        }
        return Ok(rational_new(0, 1));
    }

    let abs_exp_numer = {
        let n = exponent.to_big_numer();
        n.try_abs()?
    };
    let abs_exp_i32 = abs_exp_numer.to_i32().ok_or(NumericFailure::Overflow)?;
    let raised = try_pow_i32(base, abs_exp_i32)?;

    let root_degree = exp_denom_u32.ok_or(NumericFailure::Overflow)?;

    let raised_n = raised.to_big_numer();
    let raised_d = raised.to_big_denom();

    let (numer_root, numer_negative) = if raised_n.is_negative() {
        if root_degree % 2 == 0 {
            return Err(NumericFailure::Irrational);
        }
        (raised_n.try_abs()?.try_nth_root(root_degree)?, true)
    } else {
        (raised_n.try_nth_root(root_degree)?, false)
    };

    let denom_root = raised_d.try_nth_root(root_degree)?;

    let numer_reconstructed = numer_root.try_pow_u32(root_degree)?;
    let denom_reconstructed = denom_root.try_pow_u32(root_degree)?;

    if numer_reconstructed != raised_n.try_abs()? {
        return Err(NumericFailure::Irrational);
    }
    if denom_reconstructed != raised_d {
        return Err(NumericFailure::Irrational);
    }

    let signed_numer = if numer_negative {
        numer_root.try_neg()?
    } else {
        numer_root
    };

    let result = try_rational_new(signed_numer, denom_root)?;

    if exponent.numer_is_negative() {
        if result.numer_is_zero() {
            return Err(NumericFailure::DivisionByZero);
        }
        let (n, d) = result.as_big_pair();
        try_rational_new(d, n)
    } else {
        Ok(result)
    }
}

pub use {
    try_add as checked_add, try_div as checked_div, try_mul as checked_mul,
    try_pow_i32 as checked_pow_i32, try_sub as checked_sub,
};

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal::Decimal;
    use std::str::FromStr;

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
            .try_pow_u32(100)
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
        let inverse = try_pow_i32(&hour_factor, -1).unwrap();
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
