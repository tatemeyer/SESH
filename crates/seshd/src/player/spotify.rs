//! Spotify, behind the [`Player`] seam.
//!
//! Everything Spotify-shaped stops here: the JSON is mapped into [`Track`] and
//! [`Playback`] at this boundary so no part of SESH above it knows what a
//! Spotify object looks like. That matters more than usual, because Spotify
//! removed a third of its Web API on 2026-03-09 and will do it again.
//!
//! The Web API does not play audio. It controls playback on a Connect device,
//! so the Pi has to *be* one — librespot, set up in Phase 6. Until then
//! [`Player::transfer`] has nothing to transfer to and will say so.

use std::path::{Path, PathBuf};

use anyhow::{anyhow, bail, Context, Result};
use std::sync::Arc;

use async_trait::async_trait;
use reqwest::{Method, StatusCode};
use serde::Deserialize;
use serde_json::Value;
use tokio::sync::Mutex;

use crate::clock::{Clock, SystemClock};

use super::auth::{self, StoredTokens};
use super::{Playback, Player, Track};

/// Most results a search may ask for.
///
/// Spotify's maximum dropped from 50 to **10** on 2026-03-09 (and the default
/// from 20 to 5). Asking for more is an error, not a silently clamped
/// request, so this is a hard ceiling rather than a preference.
pub const SEARCH_LIMIT: usize = 10;

/// Seconds of slack before an access token is treated as expired.
const EXPIRY_SKEW_S: i64 = 60;

/// How many times one call may be sent, counting refresh and rate-limit retries.
const MAX_ATTEMPTS: usize = 3;

/// Longest a rate-limit backoff will be honoured before giving up.
///
/// Spotify can answer a hammered endpoint with a `Retry-After` measured in
/// hours. A room device must not sit on a locked mutex until then.
const MAX_RETRY_AFTER_S: u64 = 30;

/// Credentials for the house account's Spotify app.
#[derive(Debug, Clone, Deserialize)]
pub struct SpotifyConfig {
    /// Client id from the Spotify developer dashboard.
    pub client_id: String,
    /// Client secret. Kept in a root-owned file, never in the log.
    pub client_secret: String,
    /// Connect device name to play through, e.g. the librespot device.
    pub device_name: String,
}

/// Read the credentials file, e.g. `/etc/sesh/spotify.toml`.
pub fn load_config(path: &Path) -> Result<SpotifyConfig> {
    let text =
        std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    parse_config(&text)
}

fn parse_config(text: &str) -> Result<SpotifyConfig> {
    let config: SpotifyConfig = toml::from_str(text).context("parsing the Spotify credentials")?;
    if config.client_id.trim().is_empty() || config.client_secret.trim().is_empty() {
        bail!("client_id and client_secret must both be set");
    }
    Ok(config)
}

/// Where the Spotify APIs live. Overridable so tests can point at a stub.
#[derive(Debug, Clone)]
pub struct Endpoints {
    /// Web API base, including the version segment.
    pub api: String,
    /// Accounts service base, for token grants.
    pub accounts: String,
}

impl Default for Endpoints {
    fn default() -> Self {
        Self {
            api: "https://api.spotify.com/v1".into(),
            accounts: auth::ACCOUNTS.into(),
        }
    }
}

#[derive(Debug)]
struct Tokens {
    refresh: String,
    access: Option<String>,
    /// Monotonic milliseconds, not wall time. An access token's life is a
    /// duration; measuring it against a clock that steps sideways at boot
    /// would have the room believe a fresh token was already stale.
    expires_at_ms: i64,
}

/// A [`Player`] backed by the Spotify Web API.
pub struct SpotifyPlayer {
    http: reqwest::Client,
    config: SpotifyConfig,
    endpoints: Endpoints,
    token_path: PathBuf,
    // Tokio's mutex, not std's: the guard is held across the `.await` that
    // refreshes, so a blocking mutex here would be a deadlock waiting to
    // happen and would not compile under clippy's await_holding_lock.
    tokens: Mutex<Tokens>,
    clock: Arc<dyn Clock>,
}

impl SpotifyPlayer {
    /// Build a player from the credentials and the stored refresh token.
    pub fn new(config: SpotifyConfig, token_path: PathBuf) -> Result<Self> {
        let stored = auth::load_tokens(&token_path)?;
        Ok(Self::with_refresh_token(
            config,
            token_path,
            stored.refresh_token,
        ))
    }

    /// Build a player from a refresh token held in memory. For tests.
    pub fn with_refresh_token(
        config: SpotifyConfig,
        token_path: PathBuf,
        refresh_token: String,
    ) -> Self {
        Self {
            http: reqwest::Client::new(),
            config,
            endpoints: Endpoints::default(),
            token_path,
            tokens: Mutex::new(Tokens {
                refresh: refresh_token,
                access: None,
                expires_at_ms: 0,
            }),
            clock: Arc::new(SystemClock::new()),
        }
    }

    /// Use `clock` instead of the real one. For tests.
    pub fn with_clock(mut self, clock: Arc<dyn Clock>) -> Self {
        self.clock = clock;
        self
    }

    /// Point this player at different hosts. For tests.
    pub fn with_endpoints(mut self, endpoints: Endpoints) -> Self {
        self.endpoints = endpoints;
        self
    }

    /// Play through a different Connect device. For tests.
    pub fn with_device_name(mut self, name: impl Into<String>) -> Self {
        self.config.device_name = name.into();
        self
    }

    /// A valid access token, minting one if the cached token is stale.
    async fn access_token(&self) -> Result<String> {
        let mut tokens = self.tokens.lock().await;
        if let Some(token) = &tokens.access {
            if self.clock.mono_ms() < tokens.expires_at_ms {
                return Ok(token.clone());
            }
        }

        let access = auth::refresh_access(
            &self.http,
            &self.endpoints.accounts,
            &self.config.client_id,
            &self.config.client_secret,
            &tokens.refresh,
        )
        .await
        .context("refreshing the Spotify access token")?;

        tokens.access = Some(access.token.clone());
        tokens.expires_at_ms =
            self.clock.mono_ms() + (access.expires_in_s - EXPIRY_SKEW_S).max(0) * 1_000;

        // Spotify occasionally rotates the refresh token. Missing this would
        // work for an hour and then lock the room out until someone
        // re-authorised by hand, with nothing pointing at the cause.
        if let Some(rotated) = access.refresh_token {
            tokens.refresh = rotated.clone();
            if let Err(error) = auth::save_tokens(
                &self.token_path,
                &StoredTokens {
                    refresh_token: rotated,
                },
            ) {
                tracing::error!(%error, "could not persist the rotated refresh token");
            }
        }

        Ok(access.token)
    }

    /// Send a request, refreshing once on 401 and backing off once on 429.
    async fn call(
        &self,
        method: Method,
        path: &str,
        query: &[(&str, String)],
        body: Option<Value>,
    ) -> Result<Option<Value>> {
        let url = format!("{}{path}", self.endpoints.api);

        for attempt in 1..=MAX_ATTEMPTS {
            let token = self.access_token().await?;
            let mut request = self
                .http
                .request(method.clone(), &url)
                .bearer_auth(token)
                .query(query);
            if let Some(body) = &body {
                request = request.json(body);
            } else if method != Method::GET {
                // Spotify's edge answers 411 Length Required to a POST or PUT
                // with no Content-Length, and reqwest omits the header entirely
                // for a bodyless request. That silently broke every call that
                // sends nothing: enqueue, skip, pause, resume — including D7's
                // pre-push, which had therefore never once worked against the
                // real API. reqwest omits the header even for a zero-length
                // body, so it has to be set by hand.
                request = request
                    .header(reqwest::header::CONTENT_LENGTH, "0")
                    .body(Vec::new());
            }

            let response = request
                .send()
                .await
                .with_context(|| format!("calling Spotify {method} {path}"))?;
            let status = response.status();

            // An expired token looks exactly like a revoked one from here, so
            // drop the cached token and let the next attempt mint a fresh one.
            // Only once: a second 401 means the grant itself is gone.
            if status == StatusCode::UNAUTHORIZED && attempt < MAX_ATTEMPTS {
                self.tokens.lock().await.access = None;
                continue;
            }

            if status == StatusCode::TOO_MANY_REQUESTS && attempt < MAX_ATTEMPTS {
                let wait = retry_after(&response).min(MAX_RETRY_AFTER_S);
                tracing::warn!(wait, "Spotify rate limited us; backing off");
                tokio::time::sleep(std::time::Duration::from_secs(wait)).await;
                continue;
            }

            // 204 is the ordinary answer to "what is playing" when nothing is,
            // and to every command that succeeded.
            if status == StatusCode::NO_CONTENT {
                return Ok(None);
            }

            let text = response.text().await.unwrap_or_default();
            if !status.is_success() {
                bail!("Spotify {method} {path} failed ({status}): {text}");
            }
            if text.trim().is_empty() {
                return Ok(None);
            }
            return Ok(Some(serde_json::from_str(&text).with_context(|| {
                format!("parsing Spotify's answer to {method} {path}")
            })?));
        }

        bail!("Spotify {method} {path} did not succeed in {MAX_ATTEMPTS} attempts")
    }

    /// The room's Connect device, or `None` if it is not there.
    ///
    /// `play` and `enqueue` name a device explicitly, because otherwise Spotify
    /// acts on whatever device the house account last used — which on a shared
    /// account is somebody's phone, and a room whose queue plays into a pocket
    /// two streets away is not a room.
    ///
    /// Falling back rather than failing: with librespot down there is no room
    /// audio at all, and refusing to play would turn a silent speaker into a
    /// broken queue. The warning names the cause.
    async fn room_device(&self) -> Option<String> {
        match self.device_id().await {
            Ok(id) => Some(id),
            Err(error) => {
                tracing::warn!(
                    %error,
                    "no room device; playing on whatever device is active instead"
                );
                None
            }
        }
    }

    /// `device_id` query parameter for the room, empty when there is no room
    /// device to name.
    async fn room_query(&self) -> Vec<(&'static str, String)> {
        match self.room_device().await {
            Some(id) => vec![("device_id", id)],
            None => Vec::new(),
        }
    }

    /// The Connect device id matching the configured device name.
    async fn device_id(&self) -> Result<String> {
        let devices = self
            .call(Method::GET, "/me/player/devices", &[], None)
            .await?
            .ok_or_else(|| anyhow!("Spotify listed no devices"))?;

        devices["devices"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|device| device["name"].as_str() == Some(self.config.device_name.as_str()))
            .and_then(|device| device["id"].as_str())
            .map(str::to_string)
            .ok_or_else(|| {
                anyhow!(
                    "no Spotify Connect device named {:?}. Is librespot running \
                     and signed in to the house account?",
                    self.config.device_name
                )
            })
    }
}

fn retry_after(response: &reqwest::Response) -> u64 {
    response
        .headers()
        .get(reqwest::header::RETRY_AFTER)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse().ok())
        // Spotify does not always send the header. One second is the
        // documented minimum and is better than hammering.
        .unwrap_or(1)
}

/// Map one Spotify track object.
///
/// Returns `None` only when there is no URI, because a track SESH cannot
/// refer to later is of no use. Everything else degrades to a default: a
/// missing title is a cosmetic problem, a dropped track is not.
pub(crate) fn track_from_json(value: &Value) -> Option<Track> {
    let uri = value["uri"].as_str()?;
    let artist = value["artists"]
        .as_array()
        .map(|artists| {
            artists
                .iter()
                .filter_map(|artist| artist["name"].as_str())
                .collect::<Vec<_>>()
                .join(", ")
        })
        .unwrap_or_default();

    Some(Track {
        uri: uri.to_string(),
        title: value["name"].as_str().unwrap_or_default().to_string(),
        artist,
        duration_ms: value["duration_ms"].as_i64().unwrap_or(0),
    })
}

/// Map the `GET /me/player` object.
pub(crate) fn playback_from_json(value: &Value) -> Option<Playback> {
    Some(Playback {
        track: track_from_json(&value["item"])?,
        progress_ms: value["progress_ms"].as_i64().unwrap_or(0),
        is_playing: value["is_playing"].as_bool().unwrap_or(false),
        device: value["device"]["name"].as_str().map(str::to_string),
    })
}

#[async_trait]
impl Player for SpotifyPlayer {
    async fn playback(&self) -> Result<Option<Playback>> {
        let Some(body) = self.call(Method::GET, "/me/player", &[], None).await? else {
            return Ok(None);
        };
        // A body with a null `item` means a device is active but idle, or is
        // playing something with no track object — an advert, typically.
        Ok(playback_from_json(&body))
    }

    async fn search(&self, query: &str) -> Result<Vec<Track>> {
        let body = self
            .call(
                Method::GET,
                "/search",
                &[
                    ("q", query.to_string()),
                    ("type", "track".into()),
                    ("limit", SEARCH_LIMIT.to_string()),
                ],
                None,
            )
            .await?;

        Ok(body
            .as_ref()
            .and_then(|body| body["tracks"]["items"].as_array())
            .into_iter()
            .flatten()
            .filter_map(track_from_json)
            .collect())
    }

    async fn enqueue(&self, uri: &str) -> Result<()> {
        let mut query = vec![("uri", uri.to_string())];
        query.extend(self.room_query().await);
        self.call(Method::POST, "/me/player/queue", &query, None)
            .await?;
        Ok(())
    }

    async fn play(&self, uri: &str) -> Result<()> {
        // `uris` rather than `context_uri`: a context is an album or playlist,
        // and handing Spotify one would let it carry on into tracks nobody in
        // the room chose once this track ends.
        self.call(
            Method::PUT,
            "/me/player/play",
            &self.room_query().await,
            Some(serde_json::json!({ "uris": [uri] })),
        )
        .await?;
        Ok(())
    }

    async fn skip(&self) -> Result<()> {
        self.call(Method::POST, "/me/player/next", &[], None)
            .await?;
        Ok(())
    }

    async fn pause(&self) -> Result<()> {
        self.call(Method::PUT, "/me/player/pause", &[], None)
            .await?;
        Ok(())
    }

    async fn resume(&self) -> Result<()> {
        // No body: an empty resume continues the current track rather than
        // restarting it or wandering into a context nobody chose.
        self.call(Method::PUT, "/me/player/play", &[], None).await?;
        Ok(())
    }

    async fn transfer(&self) -> Result<()> {
        let device = self.device_id().await?;
        self.call(
            Method::PUT,
            "/me/player",
            &[],
            Some(serde_json::json!({ "device_ids": [device], "play": false })),
        )
        .await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_full_track_maps_across() {
        let track = track_from_json(&json!({
            "uri": "spotify:track:a",
            "name": "Teenage Dirtbag",
            "artists": [{ "name": "Wheatus" }],
            "duration_ms": 235_000
        }))
        .unwrap();

        assert_eq!(track.uri, "spotify:track:a");
        assert_eq!(track.title, "Teenage Dirtbag");
        assert_eq!(track.artist, "Wheatus");
        assert_eq!(track.duration_ms, 235_000);
    }

    #[test]
    fn several_artists_are_joined() {
        let track = track_from_json(&json!({
            "uri": "spotify:track:a",
            "artists": [{ "name": "Wheatus" }, { "name": "Someone Else" }]
        }))
        .unwrap();
        assert_eq!(track.artist, "Wheatus, Someone Else");
    }

    // Metadata is cosmetic; a URI is not. Dropping a track because it had no
    // title would make a song unplayable over a missing string.
    #[test]
    fn missing_metadata_degrades_rather_than_dropping_the_track() {
        let track = track_from_json(&json!({ "uri": "spotify:track:a" })).unwrap();
        assert_eq!(track.title, "");
        assert_eq!(track.artist, "");
        assert_eq!(track.duration_ms, 0);
    }

    #[test]
    fn a_track_with_no_uri_is_dropped() {
        assert_eq!(track_from_json(&json!({ "name": "Nameless" })), None);
        assert_eq!(track_from_json(&Value::Null), None);
    }

    #[test]
    fn playing_state_maps_across() {
        let playback = playback_from_json(&json!({
            "is_playing": true,
            "progress_ms": 42_000,
            "device": { "name": "SESH" },
            "item": { "uri": "spotify:track:a", "name": "A", "duration_ms": 235_000 }
        }))
        .unwrap();

        assert!(playback.is_playing);
        assert_eq!(playback.progress_ms, 42_000);
        assert_eq!(playback.device.as_deref(), Some("SESH"));
        assert_eq!(playback.remaining_ms(), 193_000);
    }

    #[test]
    fn paused_state_maps_across() {
        let playback = playback_from_json(&json!({
            "is_playing": false,
            "progress_ms": 1_000,
            "item": { "uri": "spotify:track:a" }
        }))
        .unwrap();

        assert!(!playback.is_playing);
        assert_eq!(playback.device, None);
    }

    // A device can be active with nothing on it, and adverts arrive with a
    // null item. Neither is an error; both are "nothing is playing".
    #[test]
    fn a_body_with_no_item_is_not_playback() {
        assert_eq!(playback_from_json(&json!({ "is_playing": false })), None);
        assert_eq!(
            playback_from_json(&json!({ "is_playing": true, "item": null })),
            None
        );
    }

    // `SEARCH_LIMIT` is not asserted here: comparing a constant against a
    // literal is a tautology, which clippy rightly rejects. What matters is
    // the value that reaches Spotify, and `tests/spotify_stub.rs` asserts the
    // request really carries `limit=10`.

    #[test]
    fn credentials_parse_from_toml() {
        let config = parse_config(
            r#"
            client_id = "abc"
            client_secret = "shh"
            device_name = "SESH"
            "#,
        )
        .unwrap();

        assert_eq!(config.client_id, "abc");
        assert_eq!(config.device_name, "SESH");
    }

    // The shipped template has empty values. Starting with it should say so,
    // not fail later inside a token request with an opaque 400.
    #[test]
    fn empty_credentials_are_rejected_early() {
        let error = parse_config(
            r#"
            client_id = ""
            client_secret = ""
            device_name = "SESH"
            "#,
        )
        .unwrap_err()
        .to_string();
        assert!(error.contains("client_id"), "got {error}");
    }

    #[test]
    fn malformed_credentials_are_rejected() {
        assert!(parse_config("not toml [[[").is_err());
        assert!(parse_config(r#"client_id = "abc""#).is_err());
    }
}
