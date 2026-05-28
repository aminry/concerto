//! `concerto-core` binary entry point.
//!
//! Real runtime supervision is filled in by Task 11; today this binary
//! initializes logging, emits a startup line, and exits cleanly.

use concerto_core::logging;
use concerto_error::Result;

fn main() -> Result<()> {
    let _log_guard = logging::init()?;
    tracing::info!("concerto-core starting");
    tracing::trace!("logging initialized");
    Ok(())
}
