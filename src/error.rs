//! Error type shared across MapsKit.

use std::fmt;

/// Errors returned by the TontooMapsKit framework.
#[derive(Debug, Clone)]
pub enum MapsError {
    /// The HTTP request failed (offline, DNS, timeout, ...).
    Network(String),
    /// The provider response could not be parsed.
    Parse(String),
    /// A provider returned an error response.
    Provider(String),
    /// All providers in the chain failed. Contains the last error.
    NoProviderAvailable(Box<MapsError>),
    /// The query string or arguments are invalid.
    InvalidQuery(String),
    /// A map tile could not be loaded.
    Tile(String),
    /// A globe rendering error.
    Render(String),
}

impl fmt::Display for MapsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            MapsError::Network(e) => write!(f, "network error: {e}"),
            MapsError::Parse(e) => write!(f, "parse error: {e}"),
            MapsError::Provider(e) => write!(f, "provider error: {e}"),
            MapsError::NoProviderAvailable(e) => {
                write!(f, "all providers failed, last error: {e}")
            }
            MapsError::InvalidQuery(e) => write!(f, "invalid query: {e}"),
            MapsError::Tile(e) => write!(f, "tile error: {e}"),
            MapsError::Render(e) => write!(f, "render error: {e}"),
        }
    }
}

impl std::error::Error for MapsError {}

impl From<networkkit::types::NetworkError> for MapsError {
    fn from(e: networkkit::types::NetworkError) -> Self {
        match e {
            networkkit::types::NetworkError::ParseError(msg) => MapsError::Parse(msg),
            other => MapsError::Network(other.to_string()),
        }
    }
}

impl From<foundation::error::FoundationError> for MapsError {
    fn from(e: foundation::error::FoundationError) -> Self {
        MapsError::Parse(e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display() {
        assert_eq!(
            MapsError::Tile("404".into()).to_string(),
            "tile error: 404"
        );
        let chained = MapsError::NoProviderAvailable(Box::new(MapsError::Parse("bad".into())));
        assert!(chained.to_string().contains("all providers failed"));
    }
}
