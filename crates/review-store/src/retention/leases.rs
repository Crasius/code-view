//! What a view holds open, so retention never pulls it away: a log shown or still downloading
//! (by path), a pull request page with its drafts (by pull request key).

use std::borrow::Borrow;
use std::collections::HashMap;
use std::hash::Hash;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, PoisonError};

type Held<K> = Arc<Mutex<HashMap<K, usize>>>;

pub struct Leases<K = PathBuf> {
    held: Held<K>,
}

impl<K> Clone for Leases<K> {
    fn clone(&self) -> Self {
        Self { held: self.held.clone() }
    }
}

impl<K> Default for Leases<K> {
    fn default() -> Self {
        Self { held: Arc::default() }
    }
}

/// Keeps its key from being deleted until dropped.
pub struct Lease<K: Hash + Eq = PathBuf> {
    key: K,
    held: Held<K>,
}

impl<K: Hash + Eq + Clone> Leases<K> {
    pub fn lease(&self, key: impl Into<K>) -> Lease<K> {
        let key = key.into();
        *self
            .held
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .entry(key.clone())
            .or_default() += 1;
        Lease { key, held: self.held.clone() }
    }

    pub(crate) fn is_leased<Q: Hash + Eq + ?Sized>(&self, key: &Q) -> bool
    where
        K: Borrow<Q>,
    {
        self.held.lock().unwrap_or_else(PoisonError::into_inner).contains_key(key)
    }
}

impl<K: Hash + Eq> Drop for Lease<K> {
    fn drop(&mut self) {
        let mut held = self.held.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(count) = held.get_mut(&self.key) {
            *count -= 1;
            if *count == 0 {
                held.remove(&self.key);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;

    #[test]
    fn a_key_is_leased_until_the_last_lease_drops() {
        let leases: Leases = Leases::default();
        let path = Path::new("logs/a.log");
        let first = leases.lease(path);
        let second = leases.lease(path);
        drop(first);
        assert!(leases.is_leased(path));
        drop(second);
        assert!(!leases.is_leased(path));
    }
}
