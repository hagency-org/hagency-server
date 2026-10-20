use serde::{Deserialize, Serialize};
use std::fmt;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct MatrixError {
    pub errcode: String,
    #[serde(default)]
    pub error: Option<String>,
}

impl fmt::Display for MatrixError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}: {}",
            self.errcode,
            self.error.as_deref().unwrap_or("")
        )
    }
}

impl std::error::Error for MatrixError {}

#[derive(Debug, Clone)]
pub struct HttpError {
    pub message: String,
    pub status: u16,
    pub body: Option<MatrixError>,
    pub request_id: Option<String>,
}

impl fmt::Display for HttpError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(ref rid) = self.request_id {
            write!(f, "{} (ref: {})", self.message, rid)
        } else {
            write!(f, "{}", self.message)
        }
    }
}

impl std::error::Error for HttpError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.body.as_ref().map(|body| body as _)
    }
}

impl HttpError {
    /// Build an `HttpError` from a free-form message with no HTTP status
    /// (used for client-side / transport / serialization failures).
    pub fn message(msg: impl Into<String>) -> Self {
        HttpError {
            message: msg.into(),
            status: 0,
            body: None,
            request_id: None,
        }
    }

    /// Build an `HttpError` carrying an HTTP status code and message.
    pub fn from_status(status: u16, msg: impl Into<String>) -> Self {
        HttpError {
            message: msg.into(),
            status,
            body: None,
            request_id: None,
        }
    }
}

pub fn display_error(errcode: &str, status: u16, message: &str) -> String {
    format!("{errcode} ({status}): {message}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::error::Error;

    #[test]
    fn http_error_preserves_matrix_source_and_request_reference() {
        let error = HttpError {
            message: "Forbidden".into(),
            status: 403,
            body: Some(MatrixError {
                errcode: "M_FORBIDDEN".into(),
                error: Some("Permission denied".into()),
            }),
            request_id: Some("request-123".into()),
        };
        assert_eq!(error.to_string(), "Forbidden (ref: request-123)");
        assert_eq!(
            error.source().unwrap().to_string(),
            "M_FORBIDDEN: Permission denied"
        );
        assert!(HttpError::message("Transport error").source().is_none());
    }
}
