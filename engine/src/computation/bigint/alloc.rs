//! Fallible allocation helpers for vendored bigint digits.

/// Allocation could not be satisfied. Only allocation failures use this type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AllocError;

pub fn try_reserve_exact<T>(vec: &mut Vec<T>, additional: usize) -> Result<(), AllocError> {
    vec.try_reserve_exact(additional).map_err(|_| AllocError)
}

pub fn try_with_capacity<T>(capacity: usize) -> Result<Vec<T>, AllocError> {
    let mut vec = Vec::new();
    try_reserve_exact(&mut vec, capacity)?;
    Ok(vec)
}

#[cfg(test)]
mod tests {
    use super::{try_reserve_exact, try_with_capacity, AllocError};

    #[test]
    fn try_reserve_exact_succeeds_for_small_additional() {
        let mut vec: Vec<u32> = Vec::new();
        try_reserve_exact(&mut vec, 8).expect("small reserve must succeed");
        assert!(vec.capacity() >= 8);
    }

    #[test]
    fn try_reserve_exact_maps_platform_failure_to_alloc_error() {
        let mut vec: Vec<u64> = Vec::new();
        // Impossible capacity: returns Err without attempting to commit that much RAM.
        assert_eq!(try_reserve_exact(&mut vec, usize::MAX), Err(AllocError));
    }

    #[test]
    fn try_with_capacity_maps_platform_failure_to_alloc_error() {
        assert_eq!(try_with_capacity::<u8>(usize::MAX), Err(AllocError));
    }
}
