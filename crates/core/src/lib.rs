//! Concerto Core daemon library.
//!
//! Hosts the long-lived runtime. Subsystems hang off of it as separate
//! modules. The runtime itself is filled in by Task 11; this crate
//! currently only carries the logging setup.

pub mod logging;
