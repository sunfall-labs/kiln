//! Content hashing for the query database.
//!
//! FNV-1a, written out by hand rather than reached for from `std::hash`. `DefaultHasher` is
//! explicitly documented as unstable across Rust releases, and these hashes end up in
//! `ModuleSummary` on the wire and (later) in an on-disk cache, so they have to mean the same
//! thing tomorrow as they do today.
//!
//! FNV is not collision resistant, which is survivable here because a hash is never the sole
//! basis for a cache decision: early cutoff compares whole values with `PartialEq`, and the
//! hashes are carried *inside* those values. A collision therefore costs an extra recompute at
//! worst, never a stale result. If a hash ever becomes the only thing compared, this needs to
//! be a real 128-bit hash instead.

const OFFSET_BASIS: u64 = 0xcbf2_9ce4_8422_2325;
const PRIME: u64 = 0x0000_0100_0000_01b3;

/// Incremental FNV-1a state, so callers can fold several pieces into one hash.
pub(crate) struct Fnv1a(u64);

impl Fnv1a {
    pub(crate) const fn new() -> Self {
        Self(OFFSET_BASIS)
    }

    pub(crate) fn write(&mut self, bytes: &[u8]) {
        for byte in bytes {
            self.0 ^= u64::from(*byte);
            self.0 = self.0.wrapping_mul(PRIME);
        }
    }

    pub(crate) fn write_u64(&mut self, value: u64) {
        self.write(&value.to_le_bytes());
    }

    pub(crate) fn finish(&self) -> u64 {
        self.0
    }
}

/// Hash of a file's full text. Changes whenever anything in the file changes, including
/// whitespace and comments - that is the point of it, it is the "did anything at all move"
/// signal that sits underneath the interface hash.
pub fn content_hash(text: &str) -> u64 {
    let mut hasher = Fnv1a::new();
    hasher.write(text.as_bytes());
    hasher.finish()
}

/// Hash of a list of exported names, and nothing else.
///
/// Names are separated by a byte that cannot occur in UTF-8, so `["ab", "c"]` and `["a", "bc"]`
/// cannot collide by concatenation. The caller decides the order; [`crate::Interface`] sorts,
/// so that moving a declaration within a file does not perturb the interface hash.
pub(crate) fn hash_names(names: &[String]) -> u64 {
    let mut hasher = Fnv1a::new();
    for name in names {
        hasher.write(name.as_bytes());
        hasher.write(&[0xff]);
    }
    hasher.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| (*s).to_string()).collect()
    }

    /// Pins the algorithm, not just its behaviour: these hashes are a wire and (eventually)
    /// on-disk format, so a change here has to be a deliberate one.
    #[test]
    fn matches_reference_fnv1a() {
        assert_eq!(content_hash(""), 0xcbf2_9ce4_8422_2325);
        assert_eq!(content_hash("hello"), 0xa430_d846_80aa_bd0b);
    }

    #[test]
    fn content_hash_is_sensitive_to_every_byte() {
        assert_ne!(content_hash("a\n"), content_hash("a\n\n"));
        assert_ne!(content_hash("// x"), content_hash("// y"));
        assert_eq!(content_hash("same"), content_hash("same"));
    }

    #[test]
    fn name_lists_cannot_collide_by_concatenation() {
        assert_ne!(
            hash_names(&names(&["ab", "c"])),
            hash_names(&names(&["a", "bc"]))
        );
        assert_ne!(hash_names(&names(&["a"])), hash_names(&names(&["a", ""])));
        assert_eq!(
            hash_names(&names(&["a", "b"])),
            hash_names(&names(&["a", "b"]))
        );
        assert_ne!(
            hash_names(&names(&["a", "b"])),
            hash_names(&names(&["b", "a"]))
        );
    }

    #[test]
    fn folding_u64s_is_order_sensitive() {
        let fold = |values: &[u64]| {
            let mut hasher = Fnv1a::new();
            for value in values {
                hasher.write_u64(*value);
            }
            hasher.finish()
        };
        assert_ne!(fold(&[1, 2]), fold(&[2, 1]));
        assert_ne!(fold(&[]), fold(&[0]));
    }
}
