//! Insertion-ordered hash map used by checker paths whose iteration order is
//! part of the source behavior. The side index keeps lookup and insertion
//! expected O(1), like Bun's `ArrayHashMap`.

use std::hash::Hash;
use std::marker::PhantomData;

/// Context marker matching Bun's default key hashing context.
#[derive(Clone, Copy, Debug, Default)]
pub struct AutoContext;

/// Insertion-ordered map. `C` and `A` are retained for source API parity;
/// this port uses Rust's standard hash builder and global map storage.
pub struct ArrayHashMap<K, V, C = AutoContext, A = std::alloc::Global> {
    entries: Vec<(K, V)>,
    index: std::collections::HashMap<K, usize>,
    _context: PhantomData<(C, A)>,
}

impl<K, V, C, A> Default for ArrayHashMap<K, V, C, A> {
    fn default() -> Self {
        Self {
            entries: Vec::new(),
            index: std::collections::HashMap::new(),
            _context: PhantomData,
        }
    }
}

impl<K: Eq + Hash + Clone, V, C, A> ArrayHashMap<K, V, C, A> {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn len(&self) -> usize {
        self.entries.len()
    }
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
    pub fn contains_key(&self, key: &K) -> bool {
        self.index.contains_key(key)
    }
    pub fn get(&self, key: &K) -> Option<&V> {
        self.index.get(key).map(|&at| &self.entries[at].1)
    }
    pub fn get_mut(&mut self, key: &K) -> Option<&mut V> {
        let at = *self.index.get(key)?;
        Some(&mut self.entries[at].1)
    }
    pub fn insert(&mut self, key: K, value: V) -> Option<V> {
        if let Some(&at) = self.index.get(&key) {
            return Some(std::mem::replace(&mut self.entries[at].1, value));
        }
        let at = self.entries.len();
        self.index.insert(key.clone(), at);
        self.entries.push((key, value));
        None
    }
    pub fn ordered_remove(&mut self, key: &K) -> Option<V> {
        let at = self.index.remove(key)?;
        let (_, value) = self.entries.remove(at);
        for i in at..self.entries.len() {
            self.index.insert(self.entries[i].0.clone(), i);
        }
        Some(value)
    }
    pub fn iter(&self) -> impl Iterator<Item = (&K, &V)> {
        self.entries.iter().map(|(key, value)| (key, value))
    }
    pub fn keys(&self) -> Vec<K> {
        self.entries.iter().map(|(key, _)| key.clone()).collect()
    }
    pub fn into_entries(self) -> (Vec<K>, Vec<V>) {
        self.entries.into_iter().unzip()
    }
}

impl<K: Eq + Hash + Clone, V, C, A> IntoIterator for ArrayHashMap<K, V, C, A> {
    type Item = (K, V);
    type IntoIter = std::vec::IntoIter<(K, V)>;
    fn into_iter(self) -> Self::IntoIter {
        self.entries.into_iter()
    }
}
