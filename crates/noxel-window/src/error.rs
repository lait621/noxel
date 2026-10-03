//! The one error type, shared by the real host and the stub.

use std::fmt;

/// Why a window could not be opened.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WindowError {
    /// The crate was built without the `window` feature.
    FeatureDisabled,
    /// The platform's event loop refused to start.
    EventLoop(String),
    /// A graphics surface could not be created for the window.
    Surface(String),
}

impl fmt::Display for WindowError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::FeatureDisabled => write!(
                f,
                "noxel-window was built without the `window` feature, so it cannot open a \
                 window. Rebuild with:\n\n    cargo run -p town-demo --features window -- \
                 --window\n\nThe engine itself has no dependencies; only this crate needs \
                 them, which is why they are opt-in (docs/adr/0002-no-dependencies.md)."
            ),
            Self::EventLoop(message) => write!(f, "the event loop could not start: {message}"),
            Self::Surface(message) => write!(f, "no drawable surface: {message}"),
        }
    }
}

impl std::error::Error for WindowError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_disabled_feature_error_explains_how_to_enable_it() {
        let message = WindowError::FeatureDisabled.to_string();
        assert!(message.contains("--features window"), "{message}");
        assert!(message.contains("cargo run"), "{message}");
        assert!(
            message.contains("no-dependencies"),
            "it should cite the ADR"
        );
    }

    #[test]
    fn every_error_has_a_message() {
        assert!(!WindowError::EventLoop("x".into()).to_string().is_empty());
        assert!(!WindowError::Surface("y".into()).to_string().is_empty());
    }

    #[test]
    fn errors_are_comparable_and_are_std_errors() {
        assert_eq!(WindowError::FeatureDisabled, WindowError::FeatureDisabled);
        assert_ne!(
            WindowError::FeatureDisabled,
            WindowError::Surface("x".into())
        );
        let boxed: Box<dyn std::error::Error> = Box::new(WindowError::FeatureDisabled);
        assert!(!boxed.to_string().is_empty());
    }
}
