//! Build script for `concerto-proto`.
//!
//! Walks `crates/proto/proto/**/*.proto` and compiles them into Rust types
//! and gRPC server/client stubs via `tonic-build`. Generated code lands in
//! `OUT_DIR`; `src/lib.rs` pulls it back via `tonic::include_proto!`.
//!
//! Every generated message also derives `serde::Serialize` /
//! `serde::Deserialize` — not used on the wire (proto encoding handles
//! that), but useful for snapshot tests and audit-log JSON serialization.
//!
//! Determinism: same `.proto` inputs produce byte-identical generated
//! output for a given tonic-build / prost-build version. We pin those
//! versions in `Cargo.toml` (tonic-build = "0.12") so the only source of
//! drift is the proto sources themselves.
//!
//! `protoc` must be on `PATH`. See `README.md` in this crate.

use std::env;
use std::path::{Path, PathBuf};
use std::{fs, io};

fn main() -> io::Result<()> {
    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"));
    let proto_root = manifest_dir.join("proto");

    // Re-run if anything under proto/ changes.
    println!("cargo:rerun-if-changed={}", proto_root.display());

    let proto_files = collect_proto_files(&proto_root)?;
    if proto_files.is_empty() {
        // Nothing to compile — bare crate, no generated modules. Should not
        // happen in practice because the placeholder file is always there
        // until Task 07 replaces it with real messages, but stay defensive.
        return Ok(());
    }

    let serde_derive = "#[derive(serde::Serialize, serde::Deserialize)]";

    // Every Timestamp field in our protos becomes `Option<prost_types::Timestamp>`
    // in Rust (proto3 message-typed fields are always optional in the
    // generated code). `prost_types::Timestamp` does not derive serde, so
    // the blanket `serde_derive` above won't compile without a `with`
    // shim. The shim lives in `src/lib.rs::serde_compat::option_timestamp`.
    //
    // Listed explicitly per `MessageName.field_name` rather than by glob,
    // because tonic-build's matcher does not let us key on field *type*.
    // When a future proto adds a Timestamp field, add a row here.
    let timestamp_fields = [
        "concerto.v1.RuntimeStatus.started_at",
        "concerto.v1.Workspace.created_at",
        "concerto.v1.Workspace.archived_at",
        "concerto.v1.Workarea.created_at",
        "concerto.v1.Workarea.last_activity_at",
        "concerto.v1.Workarea.archived_at",
        "concerto.v1.Session.started_at",
        "concerto.v1.Session.ended_at",
        // Task 18.
        "concerto.v1.Repository.last_fetch_at",
        // Task 23 — Streams `Event.at` is a per-event publish timestamp;
        // every `Event` carries one regardless of which `body` variant
        // is set.
        "concerto.v1.Event.at",
    ];

    let mut builder = tonic_build::configure()
        .build_server(true)
        .build_client(true)
        // `compile_well_known_types(false)` (the default) makes tonic-build
        // auto-map `.google.protobuf.*` to `::prost_types::*`, which is what
        // we want — downstream crates use `prost_types::Timestamp` etc.
        // directly without re-export gymnastics.
        .type_attribute(".", serde_derive)
        .out_dir(env::var("OUT_DIR").expect("OUT_DIR"));

    for field in timestamp_fields {
        builder = builder.field_attribute(
            field,
            "#[serde(with = \"crate::serde_compat::option_timestamp\")]",
        );
    }

    // `ConcertoError.fields` is `Option<google.protobuf.Struct>` → maps to
    // `Option<prost_types::Struct>` in Rust. Same serde issue as Timestamp;
    // shimmed via `serde_compat::option_struct`.
    builder = builder.field_attribute(
        "concerto.v1.ConcertoError.fields",
        "#[serde(with = \"crate::serde_compat::option_struct\")]",
    );

    builder
        .compile_protos(
            &proto_files.iter().map(|p| p.as_path()).collect::<Vec<_>>(),
            &[proto_root.as_path()],
        )
        .map_err(|e| io::Error::other(format!("tonic-build failed: {e}")))?;

    Ok(())
}

/// Recursively collect every `*.proto` file under `root`, sorted by path
/// so the compile order is deterministic.
fn collect_proto_files(root: &Path) -> io::Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    if !root.exists() {
        return Ok(out);
    }
    walk(root, &mut out)?;
    out.sort();
    Ok(out)
}

fn walk(dir: &Path, out: &mut Vec<PathBuf>) -> io::Result<()> {
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        let file_type = entry.file_type()?;
        if file_type.is_dir() {
            walk(&path, out)?;
        } else if file_type.is_file() && path.extension().and_then(|e| e.to_str()) == Some("proto")
        {
            out.push(path);
        }
    }
    Ok(())
}
