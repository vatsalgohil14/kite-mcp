use thiserror::Error;

pub type Result<T, E = KiteError> = std::result::Result<T, E>;

#[derive(Debug, Error)]
pub enum KiteError {
    /// Error envelope returned by Kite (`{"status":"error","error_type":...}`).
    #[error("{error_type}: {message}")]
    Api {
        status: u16,
        error_type: String,
        message: String,
    },

    #[error("not logged in to Kite. Call the `login` tool first.")]
    NotLoggedIn,

    #[error("configuration error: {0}")]
    Config(String),

    #[error("invalid input: {0}")]
    InvalidInput(String),

    #[error("not found: {0}")]
    NotFound(String),

    #[error("login failed: {0}")]
    Login(String),

    #[error("HTTP error: {0}")]
    Http(#[from] reqwest::Error),

    #[error("unexpected response: {0}")]
    Decode(String),

    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
}

impl KiteError {
    /// True when the access token is expired or invalidated and the user must log in again.
    pub fn is_token_error(&self) -> bool {
        matches!(self, KiteError::NotLoggedIn)
            || matches!(self, KiteError::Api { error_type, .. } if error_type == "TokenException")
    }

    /// True when the API key's plan does not include the endpoint (e.g. market data on
    /// the free Personal plan).
    pub fn is_permission_error(&self) -> bool {
        matches!(self, KiteError::Api { error_type, .. } if error_type == "PermissionException")
    }
}
