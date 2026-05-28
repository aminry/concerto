//! Concerto shared error and result types.
//!
//! Every crate in the workspace uses [`Result`] and [`Error`] for the types
//! it exposes at module boundaries (per design/00 §7.3). Crates may keep
//! private error types internal to themselves; only the boundary type must
//! come from here.

pub mod api;
mod error;

pub use crate::api::{Error, Result};
