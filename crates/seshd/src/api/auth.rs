//! Resolving a phone's bearer token to a person.
//!
//! Applied only to endpoints that must know *who* is acting. It is deliberately
//! **not** applied to the read surface or to `POST /api/events`: that endpoint
//! is the documented open ingest port, and narrowing it would defeat its
//! purpose. The TV surface holds no token and needs none.

use axum::async_trait;
use axum::extract::FromRequestParts;
use axum::http::request::Parts;
use axum::http::{header, StatusCode};

use crate::store::Person;

use super::AppState;

/// A request that carried a valid token, and who it belongs to.
///
/// Extracting this is the authentication: a handler that takes it can only be
/// reached by a known phone, and it cannot forget to check.
#[derive(Debug, Clone)]
pub struct Authenticated(pub Person);

#[async_trait]
impl FromRequestParts<AppState> for Authenticated {
    type Rejection = StatusCode;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let token = parts
            .headers
            .get(header::AUTHORIZATION)
            .and_then(|value| value.to_str().ok())
            .and_then(bearer)
            .ok_or(StatusCode::UNAUTHORIZED)?;

        let person = state
            .room
            .person_by_token(token)
            .map_err(|error| {
                tracing::error!(%error, "looking up a token failed");
                StatusCode::INTERNAL_SERVER_ERROR
            })?
            // An unknown token is 401, the same as no token at all. A phone
            // whose token was wiped needs to rescan either way, and saying
            // which of the two it was tells an attacker whether they guessed.
            .ok_or(StatusCode::UNAUTHORIZED)?;

        Ok(Self(person))
    }
}

/// The token out of an `Authorization` header, if it is a bearer one.
///
/// RFC 7235 makes the scheme case-insensitive, and clients do vary.
fn bearer(header: &str) -> Option<&str> {
    let (scheme, token) = header.split_once(' ')?;
    scheme
        .eq_ignore_ascii_case("bearer")
        .then(|| token.trim())
        .filter(|token| !token.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_bearer_header_yields_its_token() {
        assert_eq!(bearer("Bearer abc123"), Some("abc123"));
    }

    #[test]
    fn the_scheme_is_case_insensitive() {
        assert_eq!(bearer("bearer abc123"), Some("abc123"));
        assert_eq!(bearer("BEARER abc123"), Some("abc123"));
    }

    #[test]
    fn surrounding_whitespace_is_trimmed() {
        assert_eq!(bearer("Bearer   abc123  "), Some("abc123"));
    }

    #[test]
    fn a_different_scheme_is_rejected() {
        assert_eq!(bearer("Basic abc123"), None);
    }

    #[test]
    fn a_header_with_no_token_is_rejected() {
        assert_eq!(bearer("Bearer"), None);
        assert_eq!(bearer("Bearer "), None);
        assert_eq!(bearer(""), None);
    }
}
