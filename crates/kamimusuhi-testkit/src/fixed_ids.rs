use std::sync::atomic::{AtomicU64, Ordering};

use kamimusuhi_core::ids::{IdGenerator, RuntimeId};

/// Sequential IDs starting from a fixed seed so golden tests are reproducible.
///
/// IDs are `seed << 64 | counter`; the counter starts at 1 so the nil ID is
/// never produced.
#[derive(Debug)]
pub struct FixedIdGenerator {
    seed: u64,
    counter: AtomicU64,
}

impl FixedIdGenerator {
    pub fn new(seed: u64) -> Self {
        Self {
            seed,
            counter: AtomicU64::new(0),
        }
    }

    /// The ID that `next_id` will return on its `n`-th call (1-based).
    pub fn nth(&self, n: u64) -> RuntimeId {
        RuntimeId::from_u128((u128::from(self.seed) << 64) | u128::from(n))
    }
}

impl Default for FixedIdGenerator {
    fn default() -> Self {
        Self::new(0xF1)
    }
}

impl IdGenerator for FixedIdGenerator {
    fn next_id(&self) -> RuntimeId {
        let n = self.counter.fetch_add(1, Ordering::SeqCst) + 1;
        self.nth(n)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_sequential_and_non_nil() {
        let generator = FixedIdGenerator::new(0xF1);
        let a = generator.next_id();
        let b = generator.next_id();
        assert!(!a.is_nil());
        assert_eq!(a, generator.nth(1));
        assert_eq!(b, generator.nth(2));
        assert_eq!(a.to_string(), "00000000000000f10000000000000001");
    }
}
