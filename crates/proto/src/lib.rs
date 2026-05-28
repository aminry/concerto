//! Concerto gRPC schema, generated from `crates/proto/proto/concerto/v1/*.proto`.
//!
//! The actual codegen happens in `build.rs` at build time (tonic-build →
//! Rust). This file just re-exports the generated module tree and provides
//! a small serde-compat shim for `google.protobuf.Timestamp` fields (since
//! `prost_types::Timestamp` itself does not implement `serde::{Serialize,
//! Deserialize}`).
//!
//! ## Layout convention
//!
//! All proto files live under `proto/concerto/v1/` and declare
//! `package concerto.v1`. The Rust import path mirrors that:
//!
//! ```ignore
//! use concerto_proto::v1::{/* messages */};
//! use concerto_proto::v1::{/* service */_server, /* service */_client};
//! ```

#![allow(clippy::all)] // generated code carries its own conventions

/// Serde shims for foreign types (currently `prost_types::Timestamp`) so the
/// blanket `#[derive(serde::Serialize, serde::Deserialize)]` on generated
/// messages compiles.
///
/// Used via `#[serde(with = "...")]` attributes injected by `build.rs` on
/// every field whose type is `Option<prost_types::Timestamp>`.
///
/// Encoded form is RFC 3339 (e.g. `"2026-05-27T17:42:01.123Z"`) for human
/// readability in snapshot tests and audit logs.
pub mod serde_compat {
    pub mod option_timestamp {
        use prost_types::Timestamp;
        use serde::{Deserialize, Deserializer, Serialize, Serializer};

        pub fn serialize<S>(value: &Option<Timestamp>, ser: S) -> Result<S::Ok, S::Error>
        where
            S: Serializer,
        {
            match value {
                Some(ts) => {
                    let pair = (ts.seconds, ts.nanos);
                    pair.serialize(ser)
                }
                None => ser.serialize_none(),
            }
        }

        pub fn deserialize<'de, D>(de: D) -> Result<Option<Timestamp>, D::Error>
        where
            D: Deserializer<'de>,
        {
            let opt: Option<(i64, i32)> = Option::deserialize(de)?;
            Ok(opt.map(|(seconds, nanos)| Timestamp { seconds, nanos }))
        }
    }

    pub mod option_struct {
        //! Serde shim for `Option<prost_types::Struct>`. Roundtrips through a
        //! `serde_json::Value::Object` rendering of the struct so audit-log
        //! JSON is human-readable. `prost_types::Struct` itself has no serde
        //! impl, so we hand-walk the `fields: BTreeMap<String, Value>` shape.
        use prost_types::value::Kind;
        use prost_types::{ListValue, Struct, Value};
        use serde::ser::{SerializeMap, SerializeSeq};
        use serde::{Deserialize, Deserializer, Serialize, Serializer};
        use serde_json::Value as Json;

        pub fn serialize<S>(value: &Option<Struct>, ser: S) -> Result<S::Ok, S::Error>
        where
            S: Serializer,
        {
            match value {
                Some(s) => {
                    let mut map = ser.serialize_map(Some(s.fields.len()))?;
                    for (k, v) in &s.fields {
                        map.serialize_entry(k, &ValueWrap(v))?;
                    }
                    map.end()
                }
                None => ser.serialize_none(),
            }
        }

        pub fn deserialize<'de, D>(de: D) -> Result<Option<Struct>, D::Error>
        where
            D: Deserializer<'de>,
        {
            let opt: Option<Json> = Option::deserialize(de)?;
            Ok(opt.map(|j| match j {
                Json::Object(map) => {
                    let fields = map
                        .into_iter()
                        .map(|(k, v)| (k, json_to_value(v)))
                        .collect();
                    Struct { fields }
                }
                _ => Struct::default(),
            }))
        }

        struct ValueWrap<'a>(&'a Value);

        impl Serialize for ValueWrap<'_> {
            fn serialize<S: Serializer>(&self, ser: S) -> Result<S::Ok, S::Error> {
                match &self.0.kind {
                    None | Some(Kind::NullValue(_)) => ser.serialize_none(),
                    Some(Kind::BoolValue(b)) => ser.serialize_bool(*b),
                    Some(Kind::NumberValue(n)) => ser.serialize_f64(*n),
                    Some(Kind::StringValue(s)) => ser.serialize_str(s),
                    Some(Kind::ListValue(ListValue { values })) => {
                        let mut seq = ser.serialize_seq(Some(values.len()))?;
                        for v in values {
                            seq.serialize_element(&ValueWrap(v))?;
                        }
                        seq.end()
                    }
                    Some(Kind::StructValue(Struct { fields })) => {
                        let mut map = ser.serialize_map(Some(fields.len()))?;
                        for (k, v) in fields {
                            map.serialize_entry(k, &ValueWrap(v))?;
                        }
                        map.end()
                    }
                }
            }
        }

        fn json_to_value(j: Json) -> Value {
            let kind = match j {
                Json::Null => Kind::NullValue(0),
                Json::Bool(b) => Kind::BoolValue(b),
                Json::Number(n) => Kind::NumberValue(n.as_f64().unwrap_or(0.0)),
                Json::String(s) => Kind::StringValue(s),
                Json::Array(arr) => Kind::ListValue(ListValue {
                    values: arr.into_iter().map(json_to_value).collect(),
                }),
                Json::Object(obj) => Kind::StructValue(Struct {
                    fields: obj
                        .into_iter()
                        .map(|(k, v)| (k, json_to_value(v)))
                        .collect(),
                }),
            };
            Value { kind: Some(kind) }
        }
    }
}

pub mod concerto {
    pub mod v1 {
        tonic::include_proto!("concerto.v1");
    }
}

pub use concerto::v1;
