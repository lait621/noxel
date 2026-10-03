//! The no-feature build.
//!
//! Without `window` the crate must still compile and link, and it must fail
//! *informatively*: a caller who forgot the feature should be told how to turn it
//! on, not handed a missing-symbol error from the linker.

use crate::{Host, WindowConfig, WindowError};

/// Always fails, with an explanation.
///
/// # Errors
/// Always returns [`WindowError::FeatureDisabled`].
pub fn run<H: Host>(_config: WindowConfig, _host: H) -> Result<(), WindowError> {
    Err(WindowError::FeatureDisabled)
}
