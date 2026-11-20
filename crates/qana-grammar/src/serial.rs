//! What serializing a certified grammar needs beyond `#[derive]`.
//!
//! A certified grammar is plain data, and its types derive serde's
//! `Serialize` and `Deserialize` so that it can be certified in one place
//! (a worker, an earlier run) and loaded in another from bytes. Three
//! kinds of field need more than the derive gives them, and this module
//! holds what they use:
//!
//! * A `&'static str` (a style class, an LSP token type, an outline kind,
//!   a conflict kind) names one of a small set of strings the toolchain
//!   spells as literals. Read back from bytes there is no literal for it
//!   to point at, so it is [`intern`]ed: each distinct string is leaked
//!   once per process, and loading the same grammar again, or another
//!   grammar with the same classes, leaks nothing more. A
//!   `Vec<&'static str>` field reads back through [`static_strs`]; a
//!   struct with a `&'static str` field of its own implements
//!   `Deserialize` by hand, through [`intern`], because serde's derive
//!   takes such a field to borrow from the input.
//! * A `HashMap` iterates in an order of its own, so the same map would
//!   encode to different bytes each time. [`sorted_map`] and
//!   [`sorted_maps`] write it in key order: equal values encode to equal
//!   bytes, which a cache keyed by the bytes, or a test comparing them,
//!   relies on.
//! * A DFA's transition rows are arrays wider than serde implements its
//!   traits for. [`rows`] writes a table of them as one flat sequence.
//!
//! The modules are used as `#[serde(with = "qana_grammar::serial::…")]`.

use serde::de::{Deserialize, Deserializer, Error as _, Visitor};
use serde::ser::{Serialize, SerializeSeq, Serializer};
use std::collections::{BTreeSet, HashMap};
use std::hash::Hash;
use std::sync::{Mutex, PoisonError};

/// Every string [`intern`] has leaked, in this process.
static INTERNED: Mutex<BTreeSet<&'static str>> = Mutex::new(BTreeSet::new());

/// `s` as a `&'static str`: the same one for every call with equal text.
/// The first call with a given text leaks a copy of it; later calls find
/// that copy and allocate nothing.
///
/// This is how a `&'static str` field is read back from bytes. In an
/// artifact qana wrote, the strings such fields hold come from small
/// closed sets (the style legend, the outline kinds), so what the process
/// leaks is bounded by those sets, however many grammars it loads.
pub fn intern(s: &str) -> &'static str {
    // A panic elsewhere while the lock was held cannot have left the set
    // half-changed (the one change made under it is a single insert), so
    // a poisoned lock is still a sound set to read and add to.
    let mut interned = INTERNED.lock().unwrap_or_else(PoisonError::into_inner);
    if let Some(&have) = interned.get(s) {
        return have;
    }
    let leaked: &'static str = Box::leak(Box::<str>::from(s));
    interned.insert(leaked);
    leaked
}

/// A string read from the input and [`intern`]ed.
struct Interned(&'static str);

impl<'de> Deserialize<'de> for Interned {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct InternVisitor;

        impl Visitor<'_> for InternVisitor {
            type Value = Interned;

            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("a string")
            }

            fn visit_str<E: serde::de::Error>(self, v: &str) -> Result<Interned, E> {
                Ok(Interned(intern(v)))
            }
        }

        deserializer.deserialize_str(InternVisitor)
    }
}

/// A `Vec<&'static str>` field, as its strings: written as a sequence of
/// strings, read back with each one [`intern`]ed.
pub mod static_strs {
    use super::*;

    /// Write the strings as a sequence.
    pub fn serialize<S: Serializer>(
        strs: &[&'static str],
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        serializer.collect_seq(strs)
    }

    /// Read a sequence of strings, each [`intern`]ed.
    pub fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Vec<&'static str>, D::Error> {
        let strs = Vec::<Interned>::deserialize(deserializer)?;
        Ok(strs.into_iter().map(|Interned(s)| s).collect())
    }
}

/// A `HashMap` field, written in key order so that equal maps encode to
/// equal bytes. Read back as any map is.
pub mod sorted_map {
    use super::*;

    /// Write the map's entries in key order.
    pub fn serialize<K, V, S>(map: &HashMap<K, V>, serializer: S) -> Result<S::Ok, S::Error>
    where
        K: Ord + Serialize,
        V: Serialize,
        S: Serializer,
    {
        let mut entries: Vec<(&K, &V)> = map.iter().collect();
        entries.sort_unstable_by(|a, b| a.0.cmp(b.0));
        serializer.collect_map(entries)
    }

    /// Read the map.
    pub fn deserialize<'de, K, V, D>(deserializer: D) -> Result<HashMap<K, V>, D::Error>
    where
        K: Deserialize<'de> + Eq + Hash,
        V: Deserialize<'de>,
        D: Deserializer<'de>,
    {
        HashMap::deserialize(deserializer)
    }
}

/// A `Vec<HashMap>` field (an LR table's rows): each map written in key
/// order, as [`sorted_map`] writes one.
pub mod sorted_maps {
    use super::*;

    /// One map, to be written in key order.
    struct Sorted<'m, K, V>(&'m HashMap<K, V>);

    impl<K: Ord + Serialize, V: Serialize> Serialize for Sorted<'_, K, V> {
        fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
            super::sorted_map::serialize(self.0, serializer)
        }
    }

    /// Write the maps in order, each one's entries in key order.
    pub fn serialize<K, V, S>(maps: &[HashMap<K, V>], serializer: S) -> Result<S::Ok, S::Error>
    where
        K: Ord + Serialize,
        V: Serialize,
        S: Serializer,
    {
        serializer.collect_seq(maps.iter().map(Sorted))
    }

    /// Read the maps.
    pub fn deserialize<'de, K, V, D>(deserializer: D) -> Result<Vec<HashMap<K, V>>, D::Error>
    where
        K: Deserialize<'de> + Eq + Hash,
        V: Deserialize<'de>,
        D: Deserializer<'de>,
    {
        Vec::deserialize(deserializer)
    }
}

/// A `Vec<[u16; N]>` field (a DFA's transition table, one row per state):
/// written as the rows' cells in order, one flat sequence, and read back
/// into rows of `N`.
pub mod rows {
    use super::*;

    /// Write every row's cells, in order, as one sequence.
    pub fn serialize<const N: usize, S: Serializer>(
        rows: &[[u16; N]],
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        let mut seq = serializer.serialize_seq(Some(rows.len() * N))?;
        for cell in rows.iter().flatten() {
            seq.serialize_element(cell)?;
        }
        seq.end()
    }

    /// Read a sequence of cells back into rows of `N`. A sequence that is
    /// not a whole number of rows is refused.
    pub fn deserialize<'de, const N: usize, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Vec<[u16; N]>, D::Error> {
        let cells = Vec::<u16>::deserialize(deserializer)?;
        let (rows, rest) = cells.as_chunks::<N>();
        if !rest.is_empty() {
            return Err(D::Error::custom(format_args!(
                "{} cells are not a whole number of rows of {N}",
                cells.len()
            )));
        }
        Ok(rows.to_vec())
    }
}

#[cfg(test)]
mod tests {
    use super::intern;

    #[test]
    fn interning_leaks_each_text_once() {
        let first = intern("qana-serial-test-class");
        let again = intern(&String::from("qana-serial-test-class"));
        assert!(std::ptr::eq(first, again), "equal texts intern to the same string");
        assert_eq!(first, "qana-serial-test-class");
        assert!(!std::ptr::eq(first, intern("qana-serial-test-other")));
    }
}
