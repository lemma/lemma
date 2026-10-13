//! Binary engine snapshot: exact postcard encoding of Context + PlanStore + limits.

use crate::engine::Engine;
use crate::error::Error;

pub(crate) mod shared;
use serde::{Deserialize, Serialize};

const MAGIC: [u8; 4] = *b"LEMS";

const CRC_LEN: usize = std::mem::size_of::<u32>();

/// CRC32C (Castagnoli) over the whole body slice, hardware-accelerated where the
/// CPU has it. One pass over the bytes rather than one digest update per varint
/// byte inside the postcard flavor.
fn body_checksum(body: &[u8]) -> u32 {
    crc32c::crc32c(body)
}

#[derive(Debug, Serialize, Deserialize)]
struct SnapshotHeader {
    magic: [u8; 4],
    engine_version: String,
}

fn snapshot_error(message: impl Into<String>) -> Error {
    Error::request(message, None::<String>)
}

fn map_postcard_ser(err: postcard::Error) -> Error {
    snapshot_error(format!("engine snapshot serialize failed: {err}"))
}

fn map_postcard_de(err: postcard::Error) -> Error {
    snapshot_error(format!("engine snapshot deserialize failed: {err}"))
}

/// Encode `engine` as header + body + little-endian CRC32 of the body.
pub(crate) fn encode(engine: &Engine) -> Result<Vec<u8>, Error> {
    let identity_scope = shared::EncodeScope::enter();
    let header = SnapshotHeader {
        magic: MAGIC,
        engine_version: env!("CARGO_PKG_VERSION").to_string(),
    };
    let mut bytes = postcard::to_allocvec(&header).map_err(map_postcard_ser)?;
    let body = postcard::to_allocvec(engine).map_err(map_postcard_ser)?;
    drop(identity_scope);
    let checksum = body_checksum(&body);
    bytes.reserve(body.len() + CRC_LEN);
    bytes.extend_from_slice(&body);
    bytes.extend_from_slice(&checksum.to_le_bytes());
    Ok(bytes)
}

/// Decode snapshot bytes into an [`Engine`]. Stale version, bad magic, CRC mismatch,
/// corrupt body, or a spec set without plans → [`Error`].
pub(crate) fn decode(bytes: &[u8]) -> Result<Engine, Error> {
    let (header, rest) =
        postcard::take_from_bytes::<SnapshotHeader>(bytes).map_err(map_postcard_de)?;
    if header.magic != MAGIC {
        return Err(snapshot_error(format!(
            "engine snapshot magic mismatch: expected {:?}, got {:?}",
            MAGIC, header.magic
        )));
    }
    let expected_version = env!("CARGO_PKG_VERSION");
    if header.engine_version != expected_version {
        return Err(snapshot_error(format!(
            "engine snapshot version mismatch: snapshot is '{}', this engine is '{expected_version}'",
            header.engine_version
        )));
    }
    let Some(body_len) = rest.len().checked_sub(CRC_LEN) else {
        return Err(snapshot_error(format!(
            "engine snapshot body too short: {} bytes after header, need at least {CRC_LEN} for the checksum",
            rest.len()
        )));
    };
    let (body, checksum_bytes) = rest.split_at(body_len);
    let stored = u32::from_le_bytes(
        checksum_bytes
            .try_into()
            .expect("BUG: split_at(len - CRC_LEN) must leave exactly CRC_LEN bytes"),
    );
    let computed = body_checksum(body);
    if stored != computed {
        return Err(snapshot_error(format!(
            "engine snapshot checksum mismatch: stored {stored:#010x}, computed {computed:#010x}"
        )));
    }
    let identity_scope = shared::DecodeScope::enter();
    let engine: Engine = postcard::from_bytes(body).map_err(map_postcard_de)?;
    drop(identity_scope);
    check_committed_store(&engine)?;
    Ok(engine)
}

/// Every non-empty spec set in `context` must have plans. Plan effectives may be a
/// finer temporal split than AST rows (unpinned dependency boundaries).
fn check_committed_store(engine: &Engine) -> Result<(), Error> {
    for (repository, by_name) in engine.context.repositories() {
        for (spec_name, spec_set) in by_name {
            if spec_set.is_empty() {
                continue;
            }
            let has_plans = engine
                .plans
                .get_plans(repository.name.as_deref(), spec_name)
                .is_some_and(|plans| !plans.is_empty());
            if !has_plans {
                return Err(snapshot_error(format!(
                    "engine snapshot has no plans for spec set '{}'",
                    SpecSetLabel {
                        repository: repository.name.as_deref(),
                        spec: spec_name,
                    }
                )));
            }
        }
    }
    Ok(())
}

struct SpecSetLabel<'a> {
    repository: Option<&'a str>,
    spec: &'a str,
}

impl std::fmt::Display for SpecSetLabel<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.repository {
            Some(repository) => write!(f, "{repository} {}", self.spec),
            None => write!(f, "{}", self.spec),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::limits::ResourceLimits;
    use crate::parsing::ast::PrimitiveKind;
    use crate::parsing::source::SourceType;
    use crate::planning::semantics::lemma_type_for_primitive;
    use std::sync::Arc;

    fn load_simple() -> Engine {
        let mut engine = Engine::with_limits(ResourceLimits::default());
        engine
            .load([(
                SourceType::Volatile,
                r#"
spec snapshot_demo
data x: number
rule y: x + 1
"#
                .to_string(),
            )])
            .expect("load must succeed");
        engine
    }

    #[test]
    fn postcard_decimal_binary_round_trips() {
        use crate::literals::Value;
        use crate::parsing::ast::UnitArg;
        use rust_decimal::Decimal;
        use std::str::FromStr;

        let v = Value::Number(Decimal::from(42));
        let bytes = postcard::to_allocvec(&v).expect("ser value");
        let back: Value = postcard::from_bytes(&bytes).expect("de value");
        assert_eq!(v, back);

        let arg = UnitArg::Factor(Decimal::from_str("0.0092").unwrap());
        let bytes = postcard::to_allocvec(&arg).expect("ser unit");
        let back: UnitArg = postcard::from_bytes(&bytes).expect("de unit");
        assert_eq!(arg, back);
    }

    #[test]
    fn encode_decode_round_trip() {
        let engine = load_simple();
        let bytes = encode(&engine).expect("encode");
        let restored = decode(&bytes).expect("decode");
        let again = encode(&restored).expect("re-encode");
        assert_eq!(bytes, again);
    }

    #[test]
    fn wrong_magic_is_error() {
        let engine = load_simple();
        let mut bytes = encode(&engine).expect("encode");
        bytes[0] = b'X';
        match decode(&bytes) {
            Ok(_) => panic!("wrong magic must fail"),
            Err(err) => assert!(
                err.message().contains("magic"),
                "unexpected: {}",
                err.message()
            ),
        }
    }

    #[test]
    fn truncated_bytes_is_error() {
        let engine = load_simple();
        let bytes = encode(&engine).expect("encode");
        match decode(&bytes[..bytes.len() / 2]) {
            Ok(_) => panic!("truncated must fail"),
            Err(err) => assert!(
                err.message().contains("checksum mismatch"),
                "unexpected: {}",
                err.message()
            ),
        }
    }

    #[test]
    fn flipped_byte_crc_is_error() {
        let engine = load_simple();
        let mut bytes = encode(&engine).expect("encode");
        let last = bytes.len() - 1;
        bytes[last] ^= 0xff;
        match decode(&bytes) {
            Ok(_) => panic!("crc mismatch must fail"),
            Err(err) => assert!(
                err.message().contains("checksum mismatch"),
                "unexpected: {}",
                err.message()
            ),
        }
    }

    #[test]
    fn flipped_body_byte_is_error() {
        let engine = load_simple();
        let mut bytes = encode(&engine).expect("encode");
        let middle = bytes.len() / 2;
        bytes[middle] ^= 0xff;
        match decode(&bytes) {
            Ok(_) => panic!("body corruption must fail"),
            Err(err) => assert!(
                err.message().contains("checksum mismatch"),
                "unexpected: {}",
                err.message()
            ),
        }
    }

    #[test]
    fn spec_set_without_plans_is_error() {
        let mut engine = load_simple();
        engine.plans = crate::planning::PlanStore::new();
        let bytes = encode(&engine).expect("encode");
        match decode(&bytes) {
            Ok(_) => panic!("spec set without plans must fail"),
            Err(err) => assert!(
                err.message()
                    .contains("no plans for spec set 'snapshot_demo'"),
                "unexpected: {}",
                err.message()
            ),
        }
    }

    #[test]
    fn header_only_is_error() {
        let header = postcard::to_allocvec(&SnapshotHeader {
            magic: MAGIC,
            engine_version: env!("CARGO_PKG_VERSION").to_string(),
        })
        .expect("header");
        match decode(&header) {
            Ok(_) => panic!("header without body must fail"),
            Err(err) => assert!(
                err.message().contains("too short"),
                "unexpected: {}",
                err.message()
            ),
        }
    }

    #[test]
    fn wrong_engine_version_is_error() {
        let engine = load_simple();
        let bytes = encode(&engine).expect("encode");
        let mut rebuilt = postcard::to_allocvec(&SnapshotHeader {
            magic: MAGIC,
            engine_version: "0.0.0-foreign".to_string(),
        })
        .expect("foreign header");
        let real_header = postcard::to_allocvec(&SnapshotHeader {
            magic: MAGIC,
            engine_version: env!("CARGO_PKG_VERSION").to_string(),
        })
        .expect("real header");
        rebuilt.extend_from_slice(&bytes[real_header.len()..]);
        match decode(&rebuilt) {
            Ok(_) => panic!("version mismatch must fail"),
            Err(err) => assert!(
                err.message().contains("version"),
                "unexpected: {}",
                err.message()
            ),
        }
    }

    fn workspace_is_repository_key(engine: &Engine) -> bool {
        let workspace = engine.context.workspace();
        engine
            .context
            .repositories()
            .keys()
            .any(|repository| Arc::ptr_eq(repository, &workspace))
    }

    #[test]
    fn restored_workspace_is_repository_key() {
        let engine = load_simple();
        assert!(
            workspace_is_repository_key(&engine),
            "precondition: workspace Arc is a repositories key"
        );
        let restored = decode(&encode(&engine).expect("encode")).expect("decode");
        assert!(
            workspace_is_repository_key(&restored),
            "restored workspace must be the same Arc as its repositories key"
        );
    }

    fn primitive_singleton_hits(engine: &Engine) -> Vec<usize> {
        let slices = engine
            .plans
            .get_plans(None, "snapshot_demo")
            .expect("plans for snapshot_demo");
        let plan = slices.values().next().expect("one temporal slice");
        [
            PrimitiveKind::Boolean,
            PrimitiveKind::Number,
            PrimitiveKind::Text,
            PrimitiveKind::Date,
            PrimitiveKind::Ratio,
            PrimitiveKind::Time,
        ]
        .into_iter()
        .map(|kind| {
            let singleton = lemma_type_for_primitive(kind);
            let in_normal_forms = plan
                .normal_forms
                .iter()
                .filter(|node| Arc::ptr_eq(&node.result_type, singleton))
                .count();
            let in_rules = plan
                .rules
                .values()
                .filter(|rule| Arc::ptr_eq(&rule.rule_type, singleton))
                .count();
            in_normal_forms + in_rules
        })
        .collect()
    }

    #[test]
    fn restored_primitive_types_are_process_singletons() {
        let engine = load_simple();
        let original = primitive_singleton_hits(&engine);
        assert!(
            original.iter().sum::<usize>() > 0,
            "precondition: plan holds primitive singleton types"
        );
        let restored = decode(&encode(&engine).expect("encode")).expect("decode");
        assert_eq!(
            primitive_singleton_hits(&restored),
            original,
            "restored plan must reference the same primitive singletons"
        );
    }
}
