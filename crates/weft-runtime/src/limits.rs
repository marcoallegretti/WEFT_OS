//! The memory and table limits of a component's store.
//!
//! The limit on linear memory covers all of a component's memories
//! together: a component made of several core modules cannot multiply it
//! by declaring more memories. A memory or table that would grow past its
//! limit does not grow, and the component sees an allocation failure.

/// The most table elements across all of a component's tables.
const MAX_TABLE_ELEMENTS: usize = 1_000_000;
/// The most core instances, memories and tables in one store.
const MAX_INSTANCES: usize = 1_000;
const MAX_MEMORIES: usize = 100;
const MAX_TABLES: usize = 1_000;

pub struct Limits {
    max_memory: usize,
    memory: usize,
    table_elements: usize,
}

impl Limits {
    /// Limits a store's linear memory to `max_memory` bytes in total.
    pub fn new(max_memory: usize) -> Self {
        Self {
            max_memory,
            memory: 0,
            table_elements: 0,
        }
    }
}

/// Accounts a growth from `current` to `desired` against `used` of `max`.
/// A growth past the memory's or table's own `maximum` fails anyway, so it
/// is refused without being counted.
fn grow(
    used: &mut usize,
    max: usize,
    current: usize,
    desired: usize,
    maximum: Option<usize>,
) -> bool {
    if maximum.is_some_and(|m| desired > m) {
        return false;
    }
    let added = desired.saturating_sub(current);
    match used.checked_add(added) {
        Some(total) if total <= max => {
            *used = total;
            true
        }
        _ => false,
    }
}

impl wasmtime::ResourceLimiter for Limits {
    fn memory_growing(
        &mut self,
        current: usize,
        desired: usize,
        maximum: Option<usize>,
    ) -> anyhow::Result<bool> {
        Ok(grow(
            &mut self.memory,
            self.max_memory,
            current,
            desired,
            maximum,
        ))
    }

    fn table_growing(
        &mut self,
        current: usize,
        desired: usize,
        maximum: Option<usize>,
    ) -> anyhow::Result<bool> {
        Ok(grow(
            &mut self.table_elements,
            MAX_TABLE_ELEMENTS,
            current,
            desired,
            maximum,
        ))
    }

    fn instances(&self) -> usize {
        MAX_INSTANCES
    }

    fn memories(&self) -> usize {
        MAX_MEMORIES
    }

    fn tables(&self) -> usize {
        MAX_TABLES
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wasmtime::ResourceLimiter;

    #[test]
    fn the_limit_covers_all_memories_together() {
        let mut limits = Limits::new(100);
        assert!(limits.memory_growing(0, 60, None).unwrap());
        // A second memory gets only what the first left.
        assert!(!limits.memory_growing(0, 60, None).unwrap());
        assert!(limits.memory_growing(0, 40, None).unwrap());
        // Growing the first one further no longer fits.
        assert!(!limits.memory_growing(60, 61, None).unwrap());
        assert_eq!(limits.memory, 100);
    }

    #[test]
    fn a_refused_growth_is_not_counted() {
        let mut limits = Limits::new(100);
        assert!(!limits.memory_growing(0, 101, None).unwrap());
        assert!(limits.memory_growing(0, 100, None).unwrap());
        assert!(!limits.memory_growing(0, usize::MAX, None).unwrap());
        // Past a memory's own maximum: refused and not counted.
        let mut limits = Limits::new(100);
        assert!(!limits.memory_growing(0, 50, Some(40)).unwrap());
        assert!(limits.memory_growing(0, 100, None).unwrap());
    }
}
