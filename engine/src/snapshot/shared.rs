//! Identity-preserving serde for shared `Arc` values inside a snapshot.
//!
//! Serde's `rc` support writes an `Arc`'s contents at every occurrence and rebuilds a
//! separate allocation per occurrence, so pointer identity and sharing are lost on restore.
//! Here each distinct allocation is written once under an id; later occurrences write
//! only the id. Process-wide primitive singletons are seeded before encoding so they
//! restore to the same allocation they had before.

use crate::parsing::ast::PrimitiveKind;
use crate::planning::semantics::{lemma_type_for_primitive, LemmaType};
use serde::de::Error as _;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::any::Any;
use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::Arc;

type AnyArc = Arc<dyn Any + Send + Sync>;

const SINGLETON_KINDS: [PrimitiveKind; 12] = [
    PrimitiveKind::Boolean,
    PrimitiveKind::Number,
    PrimitiveKind::Text,
    PrimitiveKind::Date,
    PrimitiveKind::DateRange,
    PrimitiveKind::Time,
    PrimitiveKind::Ratio,
    PrimitiveKind::Measure,
    PrimitiveKind::MeasureRange,
    PrimitiveKind::NumberRange,
    PrimitiveKind::RatioRange,
    PrimitiveKind::TimeRange,
];

fn singleton_arc(kind: PrimitiveKind) -> &'static Arc<LemmaType> {
    lemma_type_for_primitive(kind)
}

struct EncodeTable {
    ids: HashMap<usize, u32>,
    next_id: u32,
}

struct DecodeTable {
    arcs: HashMap<u32, AnyArc>,
}

thread_local! {
    static ENCODE: RefCell<Option<EncodeTable>> = const { RefCell::new(None) };
    static DECODE: RefCell<Option<DecodeTable>> = const { RefCell::new(None) };
}

fn ptr_key<T>(arc: &Arc<T>) -> usize {
    Arc::as_ptr(arc) as usize
}

/// Active identity table for one snapshot encode. Dropping it ends the scope.
pub(crate) struct EncodeScope;

impl EncodeScope {
    pub(crate) fn enter() -> Self {
        ENCODE.with(|cell| {
            let mut slot = cell.borrow_mut();
            assert!(slot.is_none(), "BUG: nested snapshot encode scope");
            let mut table = EncodeTable {
                ids: HashMap::new(),
                next_id: 0,
            };
            for kind in SINGLETON_KINDS {
                let id = table.next_id;
                table.ids.insert(ptr_key(singleton_arc(kind)), id);
                table.next_id += 1;
            }
            *slot = Some(table);
        });
        Self
    }
}

impl Drop for EncodeScope {
    fn drop(&mut self) {
        ENCODE.with(|cell| {
            cell.borrow_mut().take();
        });
    }
}

/// Active identity table for one snapshot decode. Dropping it ends the scope.
pub(crate) struct DecodeScope;

impl DecodeScope {
    pub(crate) fn enter() -> Self {
        DECODE.with(|cell| {
            let mut slot = cell.borrow_mut();
            assert!(slot.is_none(), "BUG: nested snapshot decode scope");
            let mut table = DecodeTable {
                arcs: HashMap::new(),
            };
            for (id, kind) in SINGLETON_KINDS.into_iter().enumerate() {
                let id = u32::try_from(id).expect("BUG: singleton count fits in u32");
                let any: AnyArc = singleton_arc(kind).clone();
                table.arcs.insert(id, any);
            }
            *slot = Some(table);
        });
        Self
    }
}

impl Drop for DecodeScope {
    fn drop(&mut self) {
        DECODE.with(|cell| {
            cell.borrow_mut().take();
        });
    }
}

#[derive(Serialize)]
enum WireRef<'a, T: Serialize> {
    Full { id: u32, value: &'a T },
    Shared(u32),
}

#[derive(Deserialize)]
enum WireOwned<T> {
    Full { id: u32, value: T },
    Shared(u32),
}

fn serialize_arc<S, T>(arc: &Arc<T>, serializer: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
    T: Serialize + Send + Sync + 'static,
{
    let key = ptr_key(arc);
    let first_id = ENCODE.with(|cell| {
        let mut slot = cell.borrow_mut();
        let table = slot
            .as_mut()
            .expect("BUG: shared Arc serialized outside a snapshot encode");
        if let Some(&id) = table.ids.get(&key) {
            return Err(id);
        }
        let id = table.next_id;
        table.next_id += 1;
        table.ids.insert(key, id);
        Ok(id)
    });
    match first_id {
        Ok(id) => WireRef::Full {
            id,
            value: arc.as_ref(),
        }
        .serialize(serializer),
        Err(id) => WireRef::<T>::Shared(id).serialize(serializer),
    }
}

fn deserialize_arc<'de, D, T>(deserializer: D) -> Result<Arc<T>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de> + Send + Sync + 'static,
{
    match WireOwned::<T>::deserialize(deserializer)? {
        WireOwned::Full { id, value } => {
            let arc = Arc::new(value);
            DECODE.with(|cell| {
                let mut slot = cell.borrow_mut();
                let table = slot
                    .as_mut()
                    .expect("BUG: shared Arc deserialized outside a snapshot decode");
                let any: AnyArc = arc.clone();
                if table.arcs.insert(id, any).is_some() {
                    return Err(D::Error::custom(format!(
                        "engine snapshot defines shared id {id} twice"
                    )));
                }
                Ok(arc)
            })
        }
        WireOwned::Shared(id) => DECODE.with(|cell| {
            let slot = cell.borrow();
            let table = slot
                .as_ref()
                .expect("BUG: shared Arc deserialized outside a snapshot decode");
            let any = table.arcs.get(&id).ok_or_else(|| {
                D::Error::custom(format!("engine snapshot references unknown shared id {id}"))
            })?;
            Arc::clone(any).downcast::<T>().map_err(|_| {
                D::Error::custom(format!(
                    "engine snapshot shared id {id} does not hold the expected type"
                ))
            })
        }),
    }
}

/// Borrowed shared `Arc` for serialization inside a snapshot.
pub(crate) struct SharedRef<'a, T>(pub(crate) &'a Arc<T>);

impl<T> Serialize for SharedRef<'_, T>
where
    T: Serialize + Send + Sync + 'static,
{
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serialize_arc(self.0, serializer)
    }
}

/// Owned shared `Arc` produced by deserialization inside a snapshot.
pub(crate) struct SharedOwned<T>(pub(crate) Arc<T>);

impl<'de, T> Deserialize<'de> for SharedOwned<T>
where
    T: Deserialize<'de> + Send + Sync + 'static,
{
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserialize_arc::<D, T>(deserializer).map(SharedOwned)
    }
}

/// Serde adapter for an `Arc<T>` field.
pub(crate) mod arc {
    use super::{deserialize_arc, serialize_arc};
    use serde::{Deserializer, Serializer};
    use std::sync::Arc;

    pub(crate) fn serialize<S, T>(value: &Arc<T>, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
        T: serde::Serialize + Send + Sync + 'static,
    {
        serialize_arc(value, serializer)
    }

    pub(crate) fn deserialize<'de, D, T>(deserializer: D) -> Result<Arc<T>, D::Error>
    where
        D: Deserializer<'de>,
        T: serde::Deserialize<'de> + Send + Sync + 'static,
    {
        deserialize_arc(deserializer)
    }
}

/// Serde adapter for a `Vec<Arc<T>>` field.
pub(crate) mod arc_vec {
    use super::{SharedOwned, SharedRef};
    use serde::{Deserialize, Deserializer, Serialize, Serializer};
    use std::sync::Arc;

    pub(crate) fn serialize<S, T>(values: &[Arc<T>], serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
        T: Serialize + Send + Sync + 'static,
    {
        serializer.collect_seq(values.iter().map(SharedRef))
    }

    pub(crate) fn deserialize<'de, D, T>(deserializer: D) -> Result<Vec<Arc<T>>, D::Error>
    where
        D: Deserializer<'de>,
        T: Deserialize<'de> + Send + Sync + 'static,
    {
        let values = Vec::<SharedOwned<T>>::deserialize(deserializer)?;
        Ok(values.into_iter().map(|value| value.0).collect())
    }
}

/// Serde adapter for a `Vec<(K, Arc<T>)>` field.
pub(crate) mod arc_pair_vec {
    use super::{SharedOwned, SharedRef};
    use serde::{Deserialize, Deserializer, Serialize, Serializer};
    use std::sync::Arc;

    pub(crate) fn serialize<S, K, T>(
        pairs: &[(K, Arc<T>)],
        serializer: S,
    ) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
        K: Serialize,
        T: Serialize + Send + Sync + 'static,
    {
        serializer.collect_seq(pairs.iter().map(|(key, value)| (key, SharedRef(value))))
    }

    pub(crate) fn deserialize<'de, D, K, T>(deserializer: D) -> Result<Vec<(K, Arc<T>)>, D::Error>
    where
        D: Deserializer<'de>,
        K: Deserialize<'de>,
        T: Deserialize<'de> + Send + Sync + 'static,
    {
        let pairs = Vec::<(K, SharedOwned<T>)>::deserialize(deserializer)?;
        Ok(pairs
            .into_iter()
            .map(|(key, value)| (key, value.0))
            .collect())
    }
}

/// Serde adapter for an `IndexMap<K, Arc<T>>` field.
pub(crate) mod arc_index_map {
    use super::{SharedOwned, SharedRef};
    use indexmap::IndexMap;
    use serde::{Deserialize, Deserializer, Serialize, Serializer};
    use std::hash::Hash;
    use std::sync::Arc;

    pub(crate) fn serialize<S, K, T>(
        map: &IndexMap<K, Arc<T>>,
        serializer: S,
    ) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
        K: Serialize,
        T: Serialize + Send + Sync + 'static,
    {
        serializer.collect_seq(map.iter().map(|(key, value)| (key, SharedRef(value))))
    }

    pub(crate) fn deserialize<'de, D, K, T>(
        deserializer: D,
    ) -> Result<IndexMap<K, Arc<T>>, D::Error>
    where
        D: Deserializer<'de>,
        K: Deserialize<'de> + Eq + Hash,
        T: Deserialize<'de> + Send + Sync + 'static,
    {
        let pairs = Vec::<(K, SharedOwned<T>)>::deserialize(deserializer)?;
        let mut map = IndexMap::with_capacity(pairs.len());
        for (key, value) in pairs {
            if map.insert(key, value.0).is_some() {
                return Err(serde::de::Error::custom(
                    "engine snapshot map repeats a key",
                ));
            }
        }
        Ok(map)
    }
}

/// Serde adapter for a `BTreeMap<K, Arc<T>>` field.
pub(crate) mod arc_map {
    use super::{SharedOwned, SharedRef};
    use serde::{Deserialize, Deserializer, Serialize, Serializer};
    use std::collections::BTreeMap;
    use std::sync::Arc;

    pub(crate) fn serialize<S, K, T>(
        map: &BTreeMap<K, Arc<T>>,
        serializer: S,
    ) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
        K: Serialize,
        T: Serialize + Send + Sync + 'static,
    {
        serializer.collect_seq(map.iter().map(|(key, value)| (key, SharedRef(value))))
    }

    pub(crate) fn deserialize<'de, D, K, T>(
        deserializer: D,
    ) -> Result<BTreeMap<K, Arc<T>>, D::Error>
    where
        D: Deserializer<'de>,
        K: Deserialize<'de> + Ord,
        T: Deserialize<'de> + Send + Sync + 'static,
    {
        let pairs = Vec::<(K, SharedOwned<T>)>::deserialize(deserializer)?;
        let mut map = BTreeMap::new();
        for (key, value) in pairs {
            if map.insert(key, value.0).is_some() {
                return Err(serde::de::Error::custom(
                    "engine snapshot map repeats a key",
                ));
            }
        }
        Ok(map)
    }
}

/// Serde adapter for an `IndexMap<Arc<R>, V>` field keyed by shared repositories.
pub(crate) mod arc_key_index_map {
    use super::{SharedOwned, SharedRef};
    use indexmap::IndexMap;
    use serde::{Deserialize, Deserializer, Serialize, Serializer};
    use std::hash::Hash;
    use std::sync::Arc;

    pub(crate) fn serialize<S, R, V>(
        map: &IndexMap<Arc<R>, V>,
        serializer: S,
    ) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
        R: Serialize + Send + Sync + 'static,
        V: Serialize,
    {
        serializer.collect_seq(map.iter().map(|(key, value)| (SharedRef(key), value)))
    }

    pub(crate) fn deserialize<'de, D, R, V>(
        deserializer: D,
    ) -> Result<IndexMap<Arc<R>, V>, D::Error>
    where
        D: Deserializer<'de>,
        R: Deserialize<'de> + Hash + Eq + Send + Sync + 'static,
        V: Deserialize<'de>,
    {
        let pairs = Vec::<(SharedOwned<R>, V)>::deserialize(deserializer)?;
        let mut map = IndexMap::with_capacity(pairs.len());
        for (key, value) in pairs {
            if map.insert(key.0, value).is_some() {
                return Err(serde::de::Error::custom(
                    "engine snapshot repository key repeats",
                ));
            }
        }
        Ok(map)
    }
}
