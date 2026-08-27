//! The runtime error vocabulary, shared by every protocol face.

/// What a resolver or middleware can fail with. Protocol layers map
/// these to their own wire forms (HTTP status codes, GraphQL error
/// extensions); resolvers never think in protocol terms.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum KayakError {
    /// The request is malformed or violates the contract.
    #[error("{0}")]
    BadRequest(String),

    /// No usable identity on the request.
    #[error("{0}")]
    Unauthorized(String),

    /// Identified, but not allowed.
    #[error("{0}")]
    Forbidden(String),

    /// The addressed instance does not exist.
    #[error("not found")]
    NotFound,

    /// The request conflicts with current state.
    #[error("{0}")]
    Conflict(String),

    /// The request body exceeds a declared ceiling. Distinct from
    /// [`KayakError::BadRequest`] because a client retries an
    /// oversized payload differently from a malformed one: it shrinks
    /// the body instead of fixing it.
    #[error("{0}")]
    PayloadTooLarge(String),

    /// The caller exceeded its consumption ceiling. Retryable after
    /// waiting, which no other refusal in this vocabulary is.
    #[error("{0}")]
    TooManyRequests(String),

    /// The service itself failed.
    #[error("{0}")]
    Internal(String),
}

impl KayakError {
    /// The HTTP status this error maps to.
    pub fn status(&self) -> u16 {
        match self {
            Self::BadRequest(_) => 400,
            Self::Unauthorized(_) => 401,
            Self::Forbidden(_) => 403,
            Self::NotFound => 404,
            Self::Conflict(_) => 409,
            Self::PayloadTooLarge(_) => 413,
            Self::TooManyRequests(_) => 429,
            Self::Internal(_) => 500,
        }
    }

    /// A stable machine-readable code, carried in GraphQL error
    /// extensions and REST error envelopes.
    pub fn code(&self) -> &'static str {
        match self {
            Self::BadRequest(_) => "bad_request",
            Self::Unauthorized(_) => "unauthorized",
            Self::Forbidden(_) => "forbidden",
            Self::NotFound => "not_found",
            Self::Conflict(_) => "conflict",
            Self::PayloadTooLarge(_) => "payload_too_large",
            Self::TooManyRequests(_) => "too_many_requests",
            Self::Internal(_) => "internal",
        }
    }
}
