//! The set of hand-expanded tree nodes, with a generation counter so the
//! flattened rows can be cached until the set changes.

use mqtt_zapp_core::NodeId;
use std::collections::HashSet;

#[derive(Debug, Default, Clone)]
pub struct Expanded {
    set: HashSet<NodeId>,
    generation: u64,
}

impl Expanded {
    // The app reads membership through `as_set` (in `visible_rows`).
    #[cfg(test)]
    pub fn contains(&self, id: &NodeId) -> bool {
        self.set.contains(id)
    }

    /// Like `HashSet::insert`: true when `id` was not in the set.
    pub fn insert(&mut self, id: NodeId) -> bool {
        self.generation += 1;
        self.set.insert(id)
    }

    /// Like `HashSet::remove`: true when `id` was in the set.
    pub fn remove(&mut self, id: &NodeId) -> bool {
        self.generation += 1;
        self.set.remove(id)
    }

    pub fn clear(&mut self) {
        self.generation += 1;
        self.set.clear();
    }

    #[cfg(test)]
    pub fn is_empty(&self) -> bool {
        self.set.is_empty()
    }

    /// Bumped by every mutating call.
    pub fn generation(&self) -> u64 {
        self.generation
    }

    pub fn as_set(&self) -> &HashSet<NodeId> {
        &self.set
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_mutation_bumps_the_generation() {
        let mut e = Expanded::default();
        let g = e.generation();
        assert!(e.insert(3));
        assert!(!e.insert(3));
        assert!(e.contains(&3));
        assert!(e.generation() > g);
        let g = e.generation();
        assert!(e.remove(&3));
        assert!(e.generation() > g);
        let g = e.generation();
        e.clear();
        assert!(e.generation() > g && e.is_empty());
    }
}
