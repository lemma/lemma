//! Fallible signed arbitrary-precision integers.

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::cmp::Ordering;
use std::fmt;
use std::str::FromStr;

use super::alloc::AllocError;
use super::biguint::BigUint;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Sign {
    Minus = -1,
    Zero = 0,
    Plus = 1,
}

#[derive(Clone, PartialEq, Eq, Hash)]
pub struct BigInt {
    sign: Sign,
    magnitude: BigUint,
}

impl Serialize for BigInt {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        (&self.sign, &self.magnitude).serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for BigInt {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let (sign, magnitude): (Sign, BigUint) = Deserialize::deserialize(deserializer)?;
        match sign {
            Sign::Zero => {
                if !magnitude.is_zero() {
                    return Err(serde::de::Error::custom(
                        "BigInt Sign::Zero requires zero magnitude",
                    ));
                }
            }
            Sign::Plus | Sign::Minus => {
                if magnitude.is_zero() {
                    return Err(serde::de::Error::custom(
                        "BigInt non-zero sign requires non-zero magnitude",
                    ));
                }
            }
        }
        Ok(Self { sign, magnitude })
    }
}

impl BigInt {
    pub fn zero() -> Self {
        Self {
            sign: Sign::Zero,
            magnitude: BigUint::zero(),
        }
    }

    pub fn one() -> Self {
        Self::from_i64(1)
    }

    pub fn from_i64(value: i64) -> Self {
        if value == 0 {
            return Self::zero();
        }
        let sign = if value < 0 { Sign::Minus } else { Sign::Plus };
        Self {
            sign,
            magnitude: BigUint::try_from_u64(value.unsigned_abs())
                .expect("BUG: i64 fits in BigUint"),
        }
    }

    pub fn from_i128(value: i128) -> Self {
        if value == 0 {
            return Self::zero();
        }
        let sign = if value < 0 { Sign::Minus } else { Sign::Plus };
        let abs = if value == i128::MIN {
            BigUint::try_from_u128(1u128 << 127).expect("BUG: i128::MIN magnitude")
        } else {
            BigUint::try_from_u128(value.unsigned_abs()).expect("BUG: i128 fits in BigUint")
        };
        Self {
            sign,
            magnitude: abs,
        }
    }

    pub fn try_from_u32(value: u32) -> Result<Self, AllocError> {
        Ok(Self {
            sign: if value == 0 { Sign::Zero } else { Sign::Plus },
            magnitude: BigUint::try_from_u32(value)?,
        })
    }

    pub fn try_from_str_radix(s: &str, radix: u32) -> Result<Self, AllocError> {
        let s = s.trim();
        if s.is_empty() {
            panic!("BUG: empty BigInt literal");
        }
        if let Some(rest) = s.strip_prefix('-') {
            let mag = BigUint::try_from_str_radix(rest, radix)?;
            if mag.is_zero() {
                Ok(Self::zero())
            } else {
                Ok(Self {
                    sign: Sign::Minus,
                    magnitude: mag,
                })
            }
        } else if let Some(rest) = s.strip_prefix('+') {
            let mag = BigUint::try_from_str_radix(rest, radix)?;
            Ok(Self {
                sign: if mag.is_zero() {
                    Sign::Zero
                } else {
                    Sign::Plus
                },
                magnitude: mag,
            })
        } else {
            let mag = BigUint::try_from_str_radix(s, radix)?;
            Ok(Self {
                sign: if mag.is_zero() {
                    Sign::Zero
                } else {
                    Sign::Plus
                },
                magnitude: mag,
            })
        }
    }

    pub fn sign(&self) -> Sign {
        self.sign
    }

    pub fn is_zero(&self) -> bool {
        self.magnitude.is_zero()
    }

    pub fn is_negative(&self) -> bool {
        matches!(self.sign, Sign::Minus)
    }

    pub fn is_positive(&self) -> bool {
        matches!(self.sign, Sign::Plus)
    }

    pub fn magnitude(&self) -> &BigUint {
        &self.magnitude
    }

    pub fn try_clone(&self) -> Result<Self, AllocError> {
        Ok(Self {
            sign: self.sign,
            magnitude: self.magnitude.try_clone()?,
        })
    }

    pub fn try_abs(&self) -> Result<Self, AllocError> {
        Ok(Self {
            sign: if self.is_zero() {
                Sign::Zero
            } else {
                Sign::Plus
            },
            magnitude: self.magnitude.try_clone()?,
        })
    }

    pub fn try_neg(&self) -> Result<Self, AllocError> {
        if self.is_zero() {
            Ok(Self::zero())
        } else {
            Ok(Self {
                sign: match self.sign {
                    Sign::Plus => Sign::Minus,
                    Sign::Minus => Sign::Plus,
                    Sign::Zero => Sign::Zero,
                },
                magnitude: self.magnitude.try_clone()?,
            })
        }
    }

    pub fn try_add(&self, other: &Self) -> Result<Self, AllocError> {
        match (self.sign, other.sign) {
            (Sign::Zero, _) => other.try_clone(),
            (_, Sign::Zero) => self.try_clone(),
            (Sign::Plus, Sign::Plus) => Ok(Self {
                sign: Sign::Plus,
                magnitude: self.magnitude.try_add(&other.magnitude)?,
            }),
            (Sign::Minus, Sign::Minus) => Ok(Self {
                sign: Sign::Minus,
                magnitude: self.magnitude.try_add(&other.magnitude)?,
            }),
            (Sign::Plus, Sign::Minus) => match self.magnitude.cmp(&other.magnitude) {
                Ordering::Less => Ok(Self {
                    sign: Sign::Minus,
                    magnitude: other.magnitude.try_sub(&self.magnitude)?,
                }),
                Ordering::Greater => Ok(Self {
                    sign: Sign::Plus,
                    magnitude: self.magnitude.try_sub(&other.magnitude)?,
                }),
                Ordering::Equal => Ok(Self::zero()),
            },
            (Sign::Minus, Sign::Plus) => match self.magnitude.cmp(&other.magnitude) {
                Ordering::Less => Ok(Self {
                    sign: Sign::Plus,
                    magnitude: other.magnitude.try_sub(&self.magnitude)?,
                }),
                Ordering::Greater => Ok(Self {
                    sign: Sign::Minus,
                    magnitude: self.magnitude.try_sub(&other.magnitude)?,
                }),
                Ordering::Equal => Ok(Self::zero()),
            },
        }
    }

    pub fn try_sub(&self, other: &Self) -> Result<Self, AllocError> {
        self.try_add(&other.try_neg()?)
    }

    pub fn try_mul(&self, other: &Self) -> Result<Self, AllocError> {
        let magnitude = self.magnitude.try_mul(&other.magnitude)?;
        let sign = match (self.sign, other.sign) {
            (Sign::Zero, _) | (_, Sign::Zero) => Sign::Zero,
            (Sign::Plus, Sign::Plus) | (Sign::Minus, Sign::Minus) => Sign::Plus,
            (Sign::Plus, Sign::Minus) | (Sign::Minus, Sign::Plus) => Sign::Minus,
        };
        Ok(Self { sign, magnitude })
    }

    pub fn try_div_rem(&self, other: &Self) -> Result<(Self, Self), AllocError> {
        if other.is_zero() {
            panic!("BUG: BigInt division by zero; callers reject zero divisors first");
        }
        let (q_mag, r_mag) = self.magnitude.try_div_rem(&other.magnitude)?;
        let q_sign = match (self.sign, other.sign) {
            (Sign::Zero, _) | (_, Sign::Zero) => Sign::Zero,
            (Sign::Plus, Sign::Plus) | (Sign::Minus, Sign::Minus) => Sign::Plus,
            (Sign::Plus, Sign::Minus) | (Sign::Minus, Sign::Plus) => Sign::Minus,
        };
        let q = Self {
            sign: if q_mag.is_zero() { Sign::Zero } else { q_sign },
            magnitude: q_mag,
        };
        let r = Self {
            sign: if r_mag.is_zero() {
                Sign::Zero
            } else if self.is_negative() {
                Sign::Minus
            } else {
                Sign::Plus
            },
            magnitude: r_mag,
        };
        Ok((q, r))
    }

    pub fn try_div_trunc(&self, other: &Self) -> Result<Self, AllocError> {
        self.try_div_rem(other).map(|(q, _)| q)
    }

    pub fn try_rem(&self, other: &Self) -> Result<Self, AllocError> {
        self.try_div_rem(other).map(|(_, r)| r)
    }

    pub fn try_gcd(&self, other: &Self) -> Result<Self, AllocError> {
        let magnitude = self.magnitude.try_gcd(&other.magnitude)?;
        let sign = if magnitude.is_zero() {
            Sign::Zero
        } else {
            Sign::Plus
        };
        Ok(Self { sign, magnitude })
    }

    pub fn try_pow(&self, exp: &BigUint) -> Result<Self, AllocError> {
        let magnitude = self.magnitude.try_pow(exp)?;
        let sign = if !exp.is_odd() || !self.is_negative() {
            if magnitude.is_zero() {
                Sign::Zero
            } else {
                Sign::Plus
            }
        } else {
            Sign::Minus
        };
        Ok(Self { sign, magnitude })
    }

    /// Signed root: odd degrees accept negative values. Even degree on a negative value is a caller bug.
    pub fn try_nth_root(&self, n: &BigUint) -> Result<Self, AllocError> {
        if self.is_negative() && !n.is_odd() {
            panic!("BUG: even root of a negative BigInt; callers take roots of magnitudes");
        }
        let root_mag = self.magnitude.try_nth_root(n)?;
        let sign = if self.is_negative() && !root_mag.is_zero() {
            Sign::Minus
        } else if root_mag.is_zero() {
            Sign::Zero
        } else {
            Sign::Plus
        };
        Ok(Self {
            sign,
            magnitude: root_mag,
        })
    }

    pub fn to_i32(&self) -> Option<i32> {
        if self.magnitude.as_digits().len() > 1 {
            return None;
        }
        let u = self.magnitude.to_u32()?;
        match self.sign {
            Sign::Zero => Some(0),
            Sign::Plus => i32::try_from(u).ok(),
            Sign::Minus => 0i32.checked_sub_unsigned(u),
        }
    }

    pub fn to_i128(&self) -> Option<i128> {
        let u = self.magnitude.to_u128()?;
        match self.sign {
            Sign::Zero => Some(0),
            Sign::Plus => i128::try_from(u).ok(),
            Sign::Minus => 0i128.checked_sub_unsigned(u),
        }
    }

    pub fn to_u32(&self) -> Option<u32> {
        if self.is_negative() {
            None
        } else {
            self.magnitude.to_u32()
        }
    }

    pub fn to_u8(&self) -> Option<u8> {
        self.to_u32().and_then(|v| u8::try_from(v).ok())
    }

    pub fn to_usize(&self) -> Option<usize> {
        self.magnitude
            .to_u128()
            .and_then(|v| usize::try_from(v).ok())
    }

    pub fn bits(&self) -> u64 {
        self.magnitude.bits()
    }
}

impl fmt::Debug for BigInt {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}

impl PartialOrd for BigInt {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(std::cmp::Ord::cmp(self, other))
    }
}

impl Ord for BigInt {
    fn cmp(&self, other: &Self) -> Ordering {
        match (self.sign, other.sign) {
            (Sign::Zero, Sign::Zero) => Ordering::Equal,
            (Sign::Zero, Sign::Plus) => Ordering::Less,
            (Sign::Zero, Sign::Minus) => Ordering::Greater,
            (Sign::Plus, Sign::Zero) => Ordering::Greater,
            (Sign::Minus, Sign::Zero) => Ordering::Less,
            (Sign::Plus, Sign::Plus) => self.magnitude.cmp(&other.magnitude),
            (Sign::Minus, Sign::Minus) => other.magnitude.cmp(&self.magnitude),
            (Sign::Plus, Sign::Minus) => Ordering::Greater,
            (Sign::Minus, Sign::Plus) => Ordering::Less,
        }
    }
}

impl fmt::Display for BigInt {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.is_zero() {
            return write!(f, "0");
        }
        if self.is_negative() {
            write!(f, "-")?;
        }
        write!(f, "{}", self.magnitude)
    }
}

impl FromStr for BigInt {
    type Err = AllocError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::try_from_str_radix(s, 10)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn to_i128_rejects_magnitudes_outside_i128_instead_of_wrapping() {
        let one = BigInt::one();
        let max = BigInt::from_i128(i128::MAX);
        let min = BigInt::from_i128(i128::MIN);
        let above_max = max.try_add(&one).unwrap();
        let below_min = min.try_sub(&one).unwrap();
        assert_eq!(max.to_i128(), Some(i128::MAX));
        assert_eq!(min.to_i128(), Some(i128::MIN));
        assert_eq!(above_max.to_i128(), None);
        assert_eq!(below_min.to_i128(), None);
        assert_eq!(above_max.try_neg().unwrap().to_i128(), Some(i128::MIN));
        let u128_max = above_max.try_add(&max).unwrap();
        assert_eq!(u128_max.to_i128(), None);
        assert_eq!(u128_max.try_neg().unwrap().to_i128(), None);
    }

    #[test]
    fn to_i32_covers_exact_i32_range() {
        assert_eq!(
            BigInt::from_i64(i64::from(i32::MIN)).to_i32(),
            Some(i32::MIN)
        );
        assert_eq!(
            BigInt::from_i64(i64::from(i32::MAX)).to_i32(),
            Some(i32::MAX)
        );
        assert_eq!(BigInt::from_i64(i64::from(i32::MIN) - 1).to_i32(), None);
        assert_eq!(BigInt::from_i64(i64::from(i32::MAX) + 1).to_i32(), None);
    }

    #[test]
    fn try_gcd_is_non_negative_and_zero_only_for_two_zeros() {
        let gcd = |a: i64, b: i64| BigInt::from_i64(a).try_gcd(&BigInt::from_i64(b)).unwrap();
        assert_eq!(gcd(0, 0), BigInt::zero());
        assert!(gcd(0, 0).is_zero() && !gcd(0, 0).is_positive());
        assert_eq!(gcd(-12, 18), BigInt::from_i64(6));
        assert_eq!(gcd(0, -7), BigInt::from_i64(7));
        assert_eq!(gcd(-9, 0), BigInt::from_i64(9));
    }

    #[test]
    fn serde_round_trip_negative() {
        let value = BigInt::from_i128(-1_000_000_000_000_000_000_000);
        let bytes = serde_json::to_vec(&value).expect("serialize");
        let restored: BigInt = serde_json::from_slice(&bytes).expect("deserialize");
        assert_eq!(restored, value);
    }

    #[test]
    fn serde_rejects_zero_sign_with_nonzero_magnitude() {
        let magnitude = BigUint::try_from_u32(1).unwrap();
        let payload = serde_json::to_vec(&(&Sign::Zero, &magnitude)).unwrap();
        match serde_json::from_slice::<BigInt>(&payload) {
            Ok(_) => panic!("zero sign with magnitude must fail"),
            Err(err) => assert!(
                err.to_string().contains("Sign::Zero"),
                "unexpected error: {err}"
            ),
        }
    }

    #[test]
    fn serde_rejects_nonzero_sign_with_zero_magnitude() {
        let magnitude = BigUint::zero();
        let payload = serde_json::to_vec(&(&Sign::Plus, &magnitude)).unwrap();
        match serde_json::from_slice::<BigInt>(&payload) {
            Ok(_) => panic!("nonzero sign with zero magnitude must fail"),
            Err(err) => assert!(
                err.to_string().contains("non-zero magnitude"),
                "unexpected error: {err}"
            ),
        }
    }
}
