//! Fallible arbitrary-precision unsigned integers (algorithms from num-bigint 0.4.6).

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::cmp::Ordering;
use std::fmt;

use super::alloc::{try_reserve_exact, try_with_capacity, AllocError};
use super::digit::{BigDigit, DoubleBigDigit, BITS};

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct BigUint {
    digits: Vec<BigDigit>,
}

impl Serialize for BigUint {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.digits.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for BigUint {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let digits = Vec::<BigDigit>::deserialize(deserializer)?;
        if digits.is_empty() {
            return Err(serde::de::Error::custom("BigUint digits must be non-empty"));
        }
        if digits.len() > 1 && digits.last() == Some(&0) {
            return Err(serde::de::Error::custom(
                "BigUint digits must not have a trailing zero",
            ));
        }
        Ok(Self { digits })
    }
}

/// Per-thread operation counts so tests can assert algorithmic work deterministically.
#[cfg(test)]
pub(crate) mod work_counters {
    use std::cell::Cell;

    thread_local! {
        static COUNTS: Cell<WorkCounts> = const { Cell::new(WorkCounts::ZERO) };
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct WorkCounts {
        pub gcd_calls: u64,
        pub gcd_steps: u64,
        pub gcd_max_operand_bits: u64,
        pub div_rem_calls: u64,
    }

    impl WorkCounts {
        const ZERO: Self = Self {
            gcd_calls: 0,
            gcd_steps: 0,
            gcd_max_operand_bits: 0,
            div_rem_calls: 0,
        };
    }

    pub fn reset() {
        COUNTS.set(WorkCounts::ZERO);
    }

    pub fn snapshot() -> WorkCounts {
        COUNTS.get()
    }

    fn update(change: impl FnOnce(&mut WorkCounts)) {
        let mut counts = COUNTS.get();
        change(&mut counts);
        COUNTS.set(counts);
    }

    pub(super) fn record_gcd_call(left_bits: u64, right_bits: u64) {
        update(|counts| {
            counts.gcd_calls += 1;
            counts.gcd_max_operand_bits =
                counts.gcd_max_operand_bits.max(left_bits).max(right_bits);
        });
    }

    pub(super) fn record_gcd_step() {
        update(|counts| counts.gcd_steps += 1);
    }

    pub(super) fn record_div_rem() {
        update(|counts| counts.div_rem_calls += 1);
    }
}

/// Most digits a `Vec<BigDigit>` can hold: its byte length is capped at `isize::MAX`.
pub const MAX_DIGITS: usize = isize::MAX as usize / std::mem::size_of::<BigDigit>();

/// True when `base^exponent` provably needs more than `MAX_DIGITS` digits, for a base
/// of `base_bits` bits. Uses the lower bound `(base_bits - 1) * exponent + 1` on the
/// result's bit length. `false` does not promise that the allocation succeeds.
fn power_exceeds_allocation(base_bits: u64, exponent: &BigUint) -> Result<bool, AllocError> {
    if base_bits <= 1 {
        return Ok(false);
    }
    let lower_bits = exponent
        .try_mul(&BigUint::try_from_u64(base_bits - 1)?)?
        .try_add(&BigUint::one())?;
    let limit_bits =
        BigUint::try_from_u64(u64::try_from(MAX_DIGITS).expect("BUG: usize digit limit fits u64"))?
            .try_mul_u32(BITS)?;
    Ok(lower_bits.cmp(&limit_bits) == Ordering::Greater)
}

impl BigUint {
    pub fn zero() -> Self {
        Self { digits: vec![0] }
    }

    pub fn one() -> Self {
        Self { digits: vec![1] }
    }

    pub fn is_zero(&self) -> bool {
        self.digits.len() == 1 && self.digits[0] == 0
    }

    pub fn is_one(&self) -> bool {
        self.digits.len() == 1 && self.digits[0] == 1
    }

    pub fn try_clone(&self) -> Result<Self, AllocError> {
        let mut digits = try_with_capacity(self.digits.len())?;
        digits.extend_from_slice(&self.digits);
        Ok(Self { digits })
    }

    pub fn try_from_u32(value: u32) -> Result<Self, AllocError> {
        Ok(Self {
            digits: vec![value],
        })
    }

    pub fn try_from_u64(value: u64) -> Result<Self, AllocError> {
        if value == 0 {
            return Ok(Self::zero());
        }
        let lo = value as BigDigit;
        let hi = (value >> BITS) as BigDigit;
        if hi == 0 {
            Ok(Self { digits: vec![lo] })
        } else {
            Ok(Self {
                digits: vec![lo, hi],
            })
        }
    }

    pub fn try_from_u128(value: u128) -> Result<Self, AllocError> {
        if value == 0 {
            return Ok(Self::zero());
        }
        let mut digits = try_with_capacity(4)?;
        let mut v = value;
        while v > 0 {
            digits.push(v as BigDigit);
            v >>= BITS;
        }
        Ok(Self { digits })
    }

    /// Parses decimal digits. Other input is a caller bug and panics; only allocation returns `Err`.
    pub fn try_from_str_radix(s: &str, radix: u32) -> Result<Self, AllocError> {
        if radix != 10 {
            panic!("BUG: BigUint literals are parsed in radix 10 only");
        }
        let s = s.trim();
        if s.is_empty() {
            panic!("BUG: empty BigUint literal");
        }
        let mut result = Self::zero();
        for ch in s.chars() {
            let digit = ch
                .to_digit(radix)
                .expect("BUG: BigUint literal holds only decimal digits");
            result = result.try_mul_u32(10)?.try_add_u32(digit)?;
        }
        Ok(result)
    }

    fn normalize(&mut self) {
        while self.digits.len() > 1 && self.digits.last() == Some(&0) {
            self.digits.pop();
        }
        if self.digits.is_empty() {
            self.digits.push(0);
        }
    }

    pub fn bits(&self) -> u64 {
        if self.is_zero() {
            return 0;
        }
        let top = self.digits[self.digits.len() - 1];
        (self.digits.len() as u64 - 1) * u64::from(BITS) + u64::from(BITS - top.leading_zeros())
    }

    pub fn cmp(&self, other: &Self) -> Ordering {
        cmp_slices(&self.digits, &other.digits)
    }

    pub fn try_add(&self, other: &Self) -> Result<Self, AllocError> {
        let max_len = self.digits.len().max(other.digits.len());
        let out_len = max_len + 1;
        let mut out = try_with_capacity(out_len)?;
        out.resize(out_len, 0);
        let mut carry: DoubleBigDigit = 0;
        for (i, out_digit) in out.iter_mut().enumerate().take(max_len) {
            carry += DoubleBigDigit::from(*self.digits.get(i).unwrap_or(&0))
                + DoubleBigDigit::from(*other.digits.get(i).unwrap_or(&0));
            *out_digit = carry as BigDigit;
            carry >>= BITS;
        }
        if carry != 0 {
            out[max_len] = carry as BigDigit;
        } else {
            out.truncate(max_len);
        }
        let mut result = Self { digits: out };
        result.normalize();
        Ok(result)
    }

    fn try_add_u32(&self, other: u32) -> Result<Self, AllocError> {
        self.try_add(&Self::try_from_u32(other)?)
    }

    pub fn try_sub(&self, other: &Self) -> Result<Self, AllocError> {
        if self.cmp(other) == Ordering::Less {
            panic!("BUG: BigUint subtraction underflow");
        }
        let len = self.digits.len();
        let mut out = try_with_capacity(len)?;
        out.extend_from_slice(&self.digits);
        sub_in_place(&mut out, &other.digits);
        let mut result = Self { digits: out };
        result.normalize();
        Ok(result)
    }

    pub fn try_mul(&self, other: &Self) -> Result<Self, AllocError> {
        if self.is_zero() || other.is_zero() {
            return Ok(Self::zero());
        }
        let a = trim_leading_zeros(&self.digits);
        let b = trim_leading_zeros(&other.digits);
        let out_len = a.len() + b.len();
        let mut out = try_with_capacity(out_len)?;
        out.resize(out_len, 0);
        schoolbook_mul(&mut out, a, b);
        let mut result = Self { digits: out };
        result.normalize();
        Ok(result)
    }

    pub fn try_mul_u32(&self, other: u32) -> Result<Self, AllocError> {
        if other == 0 || self.is_zero() {
            return Ok(Self::zero());
        }
        let mut digits = try_with_capacity(self.digits.len())?;
        digits.extend_from_slice(&self.digits);
        mul_in_place(&mut digits, other as BigDigit)?;
        let mut result = Self { digits };
        result.normalize();
        Ok(result)
    }

    pub fn try_div_rem(&self, other: &Self) -> Result<(Self, Self), AllocError> {
        #[cfg(test)]
        work_counters::record_div_rem();
        if other.is_zero() {
            panic!("BUG: BigUint division by zero; callers reject zero divisors first");
        }
        if self.is_zero() {
            return Ok((Self::zero(), Self::zero()));
        }
        match self.cmp(other) {
            Ordering::Less => return Ok((Self::zero(), self.try_clone()?)),
            Ordering::Equal => return Ok((Self::one(), Self::zero())),
            Ordering::Greater => {}
        }
        if other.digits.len() == 1 {
            return div_rem_digit(self.try_clone()?, other.digits[0]);
        }
        let shift = other
            .digits
            .last()
            .expect("BUG: non-empty divisor")
            .leading_zeros();
        let (q, r) = if shift == 0 {
            div_rem_core(self.try_clone()?, &other.digits)?
        } else {
            let shifted_dividend = self.try_shl(u64::from(shift))?;
            let shifted_divisor = other.try_shl(u64::from(shift))?;
            let (q, r) = div_rem_core(shifted_dividend, &shifted_divisor.digits)?;
            (q, r.try_shr(shift)?)
        };
        Ok((q, r))
    }

    /// Binary gcd: shifts and subtractions in place. When the operands differ in digit
    /// length, one remainder step first brings the larger below the smaller, so the binary
    /// loop takes at most `2 * bits(smaller)` steps.
    pub fn try_gcd(&self, other: &Self) -> Result<Self, AllocError> {
        #[cfg(test)]
        work_counters::record_gcd_call(self.bits(), other.bits());
        if self.is_zero() {
            return other.try_clone();
        }
        if other.is_zero() {
            return self.try_clone();
        }
        let (larger, smaller) = if self.cmp(other) == Ordering::Less {
            (other, self)
        } else {
            (self, other)
        };
        let mut a = if larger.digits.len() > smaller.digits.len() {
            let (_, remainder) = larger.try_div_rem(smaller)?;
            if remainder.is_zero() {
                return smaller.try_clone();
            }
            remainder
        } else {
            larger.try_clone()?
        };
        let mut b = smaller.try_clone()?;
        let a_zeros = a.trailing_zeros();
        let common_zeros = a_zeros.min(b.trailing_zeros());
        a.shr_in_place(a_zeros);
        loop {
            #[cfg(test)]
            work_counters::record_gcd_step();
            b.shr_in_place(b.trailing_zeros());
            if a.cmp(&b) == Ordering::Greater {
                std::mem::swap(&mut a, &mut b);
            }
            sub_in_place(&mut b.digits, &a.digits);
            b.normalize();
            if b.is_zero() {
                return a.try_shl(common_zeros);
            }
        }
    }

    /// Number of trailing zero bits. `self` must be non-zero.
    fn trailing_zeros(&self) -> u64 {
        let zero_digits = self
            .digits
            .iter()
            .position(|&digit| digit != 0)
            .expect("BUG: trailing_zeros of zero");
        u64::try_from(zero_digits).expect("BUG: digit count fits u64") * u64::from(BITS)
            + u64::from(self.digits[zero_digits].trailing_zeros())
    }

    fn shr_in_place(&mut self, bits: u64) {
        let digit_shift = usize::try_from(bits / u64::from(BITS))
            .expect("BUG: shift is at most the bit length of an existing value");
        if digit_shift >= self.digits.len() {
            self.digits.truncate(1);
            self.digits[0] = 0;
            return;
        }
        self.digits.drain(..digit_shift);
        let bit_shift = (bits % u64::from(BITS)) as u32;
        if bit_shift != 0 {
            let len = self.digits.len();
            for i in 0..len {
                let high = if i + 1 < len {
                    self.digits[i + 1] << (BITS - bit_shift)
                } else {
                    0
                };
                self.digits[i] = (self.digits[i] >> bit_shift) | high;
            }
        }
        self.normalize();
    }

    pub fn bit(&self, index: u64) -> bool {
        let digit = self.digits
            [usize::try_from(index / u64::from(BITS)).expect("BUG: bit index within digits")];
        (digit >> (index % u64::from(BITS))) & 1 == 1
    }

    pub fn is_odd(&self) -> bool {
        self.digits[0] & 1 == 1
    }

    pub fn try_pow(&self, exponent: &Self) -> Result<Self, AllocError> {
        if exponent.is_zero() {
            return Self::try_from_u32(1);
        }
        if power_exceeds_allocation(self.bits(), exponent)? {
            return Err(AllocError);
        }
        let total_bits = exponent.bits();
        let mut base = self.try_clone()?;
        let mut result = Self::one();
        for index in 0..total_bits {
            if exponent.bit(index) {
                result = result.try_mul(&base)?;
            }
            if index + 1 < total_bits {
                base = base.try_mul(&base)?;
            }
        }
        Ok(result)
    }

    /// Floor of the `n`-th root. `n` must be at least 1.
    pub fn try_nth_root(&self, n: &Self) -> Result<Self, AllocError> {
        if n.is_zero() {
            panic!("BUG: root degree must be at least 1");
        }
        if self.is_zero() {
            return Ok(Self::zero());
        }
        if n.is_one() || self.is_one() {
            return self.try_clone();
        }
        if n.cmp(&Self::try_from_u128(u128::from(self.bits()))?) == Ordering::Greater {
            return Self::try_from_u32(1);
        }
        let degree = n
            .to_u128()
            .expect("BUG: root degree is at most the bit length of the value");
        let two = Self::try_from_u32(2)?;
        let root_bits_bound = Self::try_from_u128(u128::from(self.bits()) / degree + 1)?;
        let mut lo = Self::zero();
        let mut hi = two.try_pow(&root_bits_bound)?;
        while lo.cmp(&hi) == Ordering::Less {
            let mid = {
                let sum = lo.try_add(&hi)?;
                let (q, _) = sum.try_div_rem(&two)?;
                q
            };
            let mid_pow = mid.try_pow(n)?;
            match mid_pow.cmp(self) {
                Ordering::Less | Ordering::Equal => {
                    let next = mid.try_add(&Self::one())?;
                    let next_pow = next.try_pow(n)?;
                    if next_pow.cmp(self) == Ordering::Greater {
                        return Ok(mid);
                    }
                    lo = next;
                }
                Ordering::Greater => {
                    hi = mid;
                }
            }
        }
        Ok(lo)
    }

    pub fn to_u32(&self) -> Option<u32> {
        if self.digits.len() == 1 {
            Some(self.digits[0])
        } else {
            None
        }
    }

    pub fn as_digits(&self) -> &[BigDigit] {
        &self.digits
    }

    pub fn try_to_string_dec(&self) -> Result<String, AllocError> {
        if self.is_zero() {
            return Ok("0".to_string());
        }
        let ten = BigUint::try_from_u32(10)?;
        let mut n = self.try_clone()?;
        let mut digits = Vec::new();
        while !n.is_zero() {
            let (q, r) = n.try_div_rem(&ten)?;
            digits.push(r.to_u32().expect("BUG: rem fits u32") as u8);
            n = q;
        }
        Ok(digits.iter().rev().map(|d| (b'0' + *d) as char).collect())
    }

    pub fn to_string_dec(&self) -> String {
        self.try_to_string_dec()
            .expect("BUG: infallible display context")
    }

    pub fn to_u128(&self) -> Option<u128> {
        if self.is_zero() {
            return Some(0);
        }
        if self.digits.len() > 4 {
            return None;
        }
        let mut result: u128 = 0;
        for (i, &d) in self.digits.iter().enumerate() {
            result |= u128::from(d) << (i * 32);
        }
        Some(result)
    }

    fn try_shl(&self, bits: u64) -> Result<Self, AllocError> {
        if bits == 0 || self.is_zero() {
            return self.try_clone();
        }
        let digit_shift = usize::try_from(bits / u64::from(BITS)).map_err(|_| AllocError)?;
        let bit_shift = (bits % u64::from(BITS)) as u32;
        let extra = usize::from(bit_shift > 0);
        let new_len = self
            .digits
            .len()
            .checked_add(digit_shift)
            .and_then(|len| len.checked_add(extra))
            .ok_or(AllocError)?;
        let mut out = try_with_capacity(new_len)?;
        out.resize(new_len, 0);
        if bit_shift == 0 {
            out[digit_shift..digit_shift + self.digits.len()].copy_from_slice(&self.digits);
        } else {
            let mut carry = 0u64;
            for (i, &d) in self.digits.iter().enumerate() {
                let v = (u64::from(d) << bit_shift) | carry;
                out[digit_shift + i] = v as BigDigit;
                carry = v >> BITS;
            }
            if carry != 0 {
                out[digit_shift + self.digits.len()] = carry as BigDigit;
            }
        }
        let mut result = Self { digits: out };
        result.normalize();
        Ok(result)
    }

    fn try_shr(&self, bits: u32) -> Result<Self, AllocError> {
        if bits == 0 || self.is_zero() {
            return self.try_clone();
        }
        let digit_shift = (bits / BITS) as usize;
        if digit_shift >= self.digits.len() {
            return Ok(Self::zero());
        }
        let bit_shift = bits % BITS;
        let mut out = try_with_capacity(self.digits.len())?;
        if bit_shift == 0 {
            out.extend_from_slice(&self.digits[digit_shift..]);
        } else {
            let mut carry = 0u64;
            for &d in self.digits.iter().rev().skip(digit_shift) {
                let v = u64::from(d);
                out.push(((v >> bit_shift) | carry) as BigDigit);
                carry = (v & ((1u64 << bit_shift) - 1)) << (BITS - bit_shift);
            }
            out.reverse();
        }
        let mut result = Self { digits: out };
        result.normalize();
        Ok(result)
    }
}

fn trim_leading_zeros(digits: &[BigDigit]) -> &[BigDigit] {
    match digits.iter().rposition(|&d| d != 0) {
        Some(i) => &digits[..=i],
        None => &digits[..1],
    }
}

fn cmp_slices(a: &[BigDigit], b: &[BigDigit]) -> Ordering {
    let a = trim_leading_zeros(a);
    let b = trim_leading_zeros(b);
    match a.len().cmp(&b.len()) {
        Ordering::Equal => a.iter().rev().cmp(b.iter().rev()),
        other => other,
    }
}

fn sub_in_place(a: &mut [BigDigit], b: &[BigDigit]) {
    let mut borrow: DoubleBigDigit = 0;
    for (i, &db) in b.iter().enumerate() {
        let ai = DoubleBigDigit::from(a[i]);
        let diff = ai
            .wrapping_sub(DoubleBigDigit::from(db))
            .wrapping_sub(borrow);
        borrow = if diff > ai { 1 } else { 0 };
        a[i] = diff as BigDigit;
    }
    for a in a.iter_mut().skip(b.len()) {
        if borrow == 0 {
            break;
        }
        let ai = DoubleBigDigit::from(*a);
        let diff = ai.wrapping_sub(borrow);
        borrow = if diff > ai { 1 } else { 0 };
        *a = diff as BigDigit;
    }
}

fn mul_in_place(digits: &mut Vec<BigDigit>, mul: BigDigit) -> Result<(), AllocError> {
    let mut carry: DoubleBigDigit = 0;
    for d in digits.iter_mut() {
        carry += DoubleBigDigit::from(*d) * DoubleBigDigit::from(mul);
        *d = carry as BigDigit;
        carry >>= BITS;
    }
    if carry != 0 {
        try_reserve_exact(digits, 1)?;
        digits.push(carry as BigDigit);
    }
    Ok(())
}

fn schoolbook_mul(out: &mut [BigDigit], a: &[BigDigit], b: &[BigDigit]) {
    for (i, &bc) in b.iter().enumerate() {
        if bc == 0 {
            continue;
        }
        let mut carry: DoubleBigDigit = 0;
        for (j, &ac) in a.iter().enumerate() {
            let idx = i + j;
            carry += DoubleBigDigit::from(out[idx])
                + DoubleBigDigit::from(ac) * DoubleBigDigit::from(bc);
            out[idx] = carry as BigDigit;
            carry >>= BITS;
        }
        if carry != 0 {
            out[i + a.len()] = carry as BigDigit;
        }
    }
}

fn div_wide(hi: BigDigit, lo: BigDigit, divisor: BigDigit) -> (BigDigit, BigDigit) {
    debug_assert!(hi < divisor);
    let lhs = (DoubleBigDigit::from(hi) << BITS) | DoubleBigDigit::from(lo);
    let rhs = DoubleBigDigit::from(divisor);
    ((lhs / rhs) as BigDigit, (lhs % rhs) as BigDigit)
}

fn div_rem_digit(mut a: BigUint, b: BigDigit) -> Result<(BigUint, BigUint), AllocError> {
    if b == 0 {
        panic!("BUG: BigUint division by zero; callers reject zero divisors first");
    }
    let mut rem = 0;
    for d in a.digits.iter_mut().rev() {
        let (q, r) = div_wide(rem, *d, b);
        *d = q;
        rem = r;
    }
    a.normalize();
    Ok((a, BigUint::try_from_u32(rem)?))
}

fn add2_in_place(a: &mut [BigDigit], b: &[BigDigit]) -> BigDigit {
    let mut carry: DoubleBigDigit = 0;
    for (i, &db) in b.iter().enumerate() {
        carry += DoubleBigDigit::from(a[i]) + DoubleBigDigit::from(db);
        a[i] = carry as BigDigit;
        carry >>= BITS;
    }
    for a in a.iter_mut().skip(b.len()) {
        if carry == 0 {
            break;
        }
        carry += DoubleBigDigit::from(*a);
        *a = carry as BigDigit;
        carry >>= BITS;
    }
    carry as BigDigit
}

fn sub_mul_digit_same_len(a: &mut [BigDigit], b: &[BigDigit], c: BigDigit) -> BigDigit {
    const MAX: BigDigit = BigDigit::MAX;
    let mut offset_carry = MAX;
    for (x, y) in a.iter_mut().zip(b) {
        let offset_sum = ((DoubleBigDigit::from(MAX) << BITS) | DoubleBigDigit::from(*x))
            - DoubleBigDigit::from(MAX)
            + DoubleBigDigit::from(offset_carry)
            - DoubleBigDigit::from(*y) * DoubleBigDigit::from(c);
        offset_carry = (offset_sum >> BITS) as BigDigit;
        *x = offset_sum as BigDigit;
    }
    MAX - offset_carry
}

fn div_rem_core(mut a: BigUint, b: &[BigDigit]) -> Result<(BigUint, BigUint), AllocError> {
    debug_assert!(a.digits.len() >= b.len() && b.len() > 1);
    debug_assert!(b.last().unwrap().leading_zeros() == 0);

    let mut a0 = 0;
    let b0 = b[b.len() - 1];
    let b1 = b[b.len() - 2];
    let q_len = a.digits.len() - b.len() + 1;
    let mut q = try_with_capacity(q_len)?;
    q.resize(q_len, 0);

    for j in (0..q_len).rev() {
        let a1 = *a.digits.last().expect("BUG: dividend digit");
        let a2 = a.digits[a.digits.len() - 2];

        let (mut q0, mut r) = if a0 < b0 {
            let (q0, r) = div_wide(a0, a1, b0);
            (q0, r as DoubleBigDigit)
        } else {
            (BigDigit::MAX, a0 as DoubleBigDigit + a1 as DoubleBigDigit)
        };

        while r <= BigDigit::MAX as DoubleBigDigit
            && ((r << BITS) | DoubleBigDigit::from(a2))
                < q0 as DoubleBigDigit * b1 as DoubleBigDigit
        {
            q0 -= 1;
            r += b0 as DoubleBigDigit;
        }

        let mut borrow = sub_mul_digit_same_len(&mut a.digits[j..], b, q0);
        if borrow > a0 {
            q0 -= 1;
            borrow -= add2_in_place(&mut a.digits[j..], b);
        }
        debug_assert!(borrow == a0);
        q[j] = q0;
        a0 = a.digits.pop().expect("BUG: dividend digit pop");
    }

    a.digits.push(a0);
    a.normalize();
    debug_assert!(cmp_slices(&a.digits, b) == Ordering::Less);

    let mut q_uint = BigUint { digits: q };
    q_uint.normalize();
    Ok((q_uint, a))
}

impl fmt::Display for BigUint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.try_to_string_dec() {
            Ok(s) => write!(f, "{s}"),
            Err(_) => f.write_str("<out of memory>"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn euclid_gcd(left: &BigUint, right: &BigUint) -> BigUint {
        let mut a = left.clone();
        let mut b = right.clone();
        while !b.is_zero() {
            let (_, r) = a.try_div_rem(&b).unwrap();
            a = b;
            b = r;
        }
        a
    }

    fn pow(base: u32, exponent: u32) -> BigUint {
        BigUint::try_from_u32(base)
            .unwrap()
            .try_pow(&BigUint::try_from_u32(exponent).unwrap())
            .unwrap()
    }

    fn product(factors: &[&BigUint]) -> BigUint {
        factors
            .iter()
            .fold(BigUint::one(), |acc, factor| acc.try_mul(factor).unwrap())
    }

    fn gcd_fixture_pairs() -> Vec<(BigUint, BigUint)> {
        let small = |value: u128| BigUint::try_from_u128(value).unwrap();
        let mersenne = |exponent: u32| pow(2, exponent).try_sub(&BigUint::one()).unwrap();
        vec![
            (small(0), small(0)),
            (small(0), small(5)),
            (small(5), small(0)),
            (small(1), small(1)),
            (small(12), small(18)),
            (small(18), small(12)),
            (small(17), small(17)),
            (small(u128::MAX), small(u128::MAX - 1)),
            (pow(2, 64), pow(2, 40)),
            (pow(2, 40), pow(2, 64)),
            (mersenne(127), mersenne(89)),
            (
                product(&[&pow(2, 100), &pow(3, 50)]),
                product(&[&pow(2, 60), &pow(5, 30)]),
            ),
            (
                product(&[&pow(7, 300), &pow(11, 200), &mersenne(61)]),
                product(&[&pow(7, 250), &pow(13, 180), &mersenne(61)]),
            ),
            (pow(3, 2000), pow(3, 1999)),
            (
                mersenne(3000),
                pow(2, 3000).try_add(&BigUint::one()).unwrap(),
            ),
        ]
    }

    #[test]
    fn try_gcd_matches_euclid_reference() {
        for (left, right) in gcd_fixture_pairs() {
            assert_eq!(
                left.try_gcd(&right).unwrap(),
                euclid_gcd(&left, &right),
                "gcd({left}, {right})"
            );
        }
    }

    #[test]
    fn try_gcd_of_cap_sized_inputs_uses_no_division_and_linear_steps() {
        let common = pow(2, 100);
        let cases = [
            (
                product(&[&common, &pow(3, 19_000)]),
                product(&[&pow(2, 200), &pow(5, 13_000)]),
                common,
            ),
            (
                pow(3, 20_000),
                pow(2, 32_767).try_sub(&BigUint::one()).unwrap(),
                BigUint::one(),
            ),
        ];
        for (left, right, expected) in cases {
            work_counters::reset();
            assert_eq!(left.try_gcd(&right).unwrap(), expected);
            let counts = work_counters::snapshot();
            assert_eq!(counts.gcd_calls, 1);
            assert!(
                counts.div_rem_calls <= 1,
                "gcd took {} long divisions, at most one balances the operands",
                counts.div_rem_calls
            );
            let bound = 2 * (left.bits() + right.bits());
            assert!(
                counts.gcd_steps < bound,
                "gcd took {} steps, bound {bound}",
                counts.gcd_steps
            );
        }
    }

    #[test]
    fn try_gcd_steps_are_bounded_by_the_smaller_operand() {
        let huge = pow(3, 20_000);
        let cases = [
            (huge.clone(), BigUint::one(), BigUint::one()),
            (BigUint::one(), huge.clone(), BigUint::one()),
            (huge.clone(), pow(3, 7), pow(3, 7)),
            (
                huge.clone(),
                pow(2, 64).try_add(&BigUint::one()).unwrap(),
                BigUint::one(),
            ),
            (
                product(&[&huge, &pow(2, 9)]),
                pow(6, 12),
                pow(6, 9).try_mul(&pow(3, 3)).unwrap(),
            ),
        ];
        for (left, right, expected) in cases {
            work_counters::reset();
            assert_eq!(left.try_gcd(&right).unwrap(), expected);
            let counts = work_counters::snapshot();
            assert!(counts.div_rem_calls <= 1);
            let bound = 2 * left.bits().min(right.bits()) + 1;
            assert!(
                counts.gcd_steps <= bound,
                "gcd took {} steps, bound {bound}",
                counts.gcd_steps
            );
        }
    }

    #[test]
    fn try_mul_small() {
        let a = BigUint::try_from_u32(12345).unwrap();
        let b = BigUint::try_from_u32(67890).unwrap();
        let c = a.try_mul(&b).unwrap();
        assert_eq!(c.to_u128(), Some(838_102_050));
    }

    #[test]
    fn div_rem_basic() {
        let a = BigUint::try_from_u32(100).unwrap();
        let b = BigUint::try_from_u32(7).unwrap();
        let (q, r) = a.try_div_rem(&b).unwrap();
        assert_eq!(q.to_u32(), Some(14));
        assert_eq!(r.to_u32(), Some(2));
    }

    #[test]
    fn nth_root_exact_and_floor_for_high_degree() {
        let thirteen = BigUint::try_from_u32(13).unwrap();
        let base = BigUint::try_from_u32(12_345).unwrap();
        let value = base.try_pow(&thirteen).unwrap();
        assert_eq!(value.try_nth_root(&thirteen).unwrap(), base);
        let just_below = value.try_sub(&BigUint::one()).unwrap();
        assert_eq!(
            just_below.try_nth_root(&thirteen).unwrap(),
            BigUint::try_from_u32(12_344).unwrap()
        );
        let two = BigUint::try_from_u32(2).unwrap();
        let degree_above_bit_length = BigUint::try_from_u32(100_000).unwrap();
        assert_eq!(
            two.try_nth_root(&degree_above_bit_length).unwrap(),
            BigUint::one()
        );
    }

    #[test]
    #[should_panic(expected = "BUG: BigUint division by zero")]
    fn try_div_rem_zero_divisor_is_a_bug() {
        let a = BigUint::try_from_u32(2).unwrap();
        let zero = BigUint::try_from_u32(0).unwrap();
        a.try_div_rem(&zero).unwrap();
    }

    #[test]
    fn try_pow_with_bignum_exponent_matches_small_exponent() {
        let base = BigUint::try_from_u32(3).unwrap();
        let exponent = BigUint::try_from_u32(40).unwrap();
        assert_eq!(
            base.try_pow(&exponent).unwrap(),
            BigUint::try_from_u128(3u128.pow(40)).unwrap()
        );
        let zero = BigUint::zero();
        assert_eq!(base.try_pow(&zero).unwrap(), BigUint::one());
    }

    #[test]
    fn power_exceeds_allocation_refuses_only_provably_unallocatable_results() {
        let two_pow_70 = BigUint::try_from_u128(1u128 << 70).unwrap();
        let two_pow_60 = BigUint::try_from_u128(1u128 << 60).unwrap();
        assert!(!power_exceeds_allocation(0, &two_pow_70).unwrap());
        assert!(!power_exceeds_allocation(1, &two_pow_70).unwrap());
        assert!(power_exceeds_allocation(2, &two_pow_70).unwrap());
        assert!(!power_exceeds_allocation(2, &two_pow_60).unwrap());
    }

    #[test]
    fn try_pow_refuses_result_beyond_max_digits() {
        let base = BigUint::try_from_u32(2).unwrap();
        let exponent = BigUint::try_from_u128(1u128 << 70).unwrap();
        assert_eq!(base.try_pow(&exponent), Err(AllocError));
    }

    #[test]
    fn try_pow_of_one_with_huge_exponent_is_one() {
        let one = BigUint::one();
        let exponent = BigUint::try_from_u128(1u128 << 100).unwrap();
        assert_eq!(one.try_pow(&exponent).unwrap(), BigUint::one());
    }

    #[test]
    fn serde_round_trip_preserves_digits() {
        let value = BigUint::try_from_u128(1u128 << 80).unwrap();
        let bytes = serde_json::to_vec(&value).expect("serialize");
        let restored: BigUint = serde_json::from_slice(&bytes).expect("deserialize");
        assert_eq!(restored, value);
    }

    #[test]
    fn serde_rejects_trailing_zero_digit() {
        match serde_json::from_str::<BigUint>("[1,0]") {
            Ok(_) => panic!("trailing zero must fail"),
            Err(err) => assert!(
                err.to_string().contains("trailing zero"),
                "unexpected error: {err}"
            ),
        }
    }

    #[test]
    fn serde_rejects_empty_digits() {
        match serde_json::from_str::<BigUint>("[]") {
            Ok(_) => panic!("empty digits must fail"),
            Err(err) => assert!(
                err.to_string().contains("non-empty"),
                "unexpected error: {err}"
            ),
        }
    }
}
