//! Concerto gRPC schema, generated from `crates/proto/proto/concerto/v1/*.proto`.
//!
//! The actual codegen happens in `build.rs` at build time (tonic-build →
//! Rust). This file just re-exports the generated module tree.
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
//!
//! Task 06 establishes only this scaffolding; the first real messages and
//! services arrive in Task 07.

#![allow(clippy::all)] // generated code carries its own conventions

pub mod concerto {
    pub mod v1 {
        tonic::include_proto!("concerto.v1");
    }
}

pub use concerto::v1;
