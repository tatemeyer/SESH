//! Authorising the house Spotify account, once.
//!
//! One account for the whole house, not one per person. That is not a
//! shortcut: a Spotify app in Development Mode allows **five** authorised
//! users, so per-person authorisation would refuse the sixth friend through
//! the door. Because only the house account ever authorises, the cap is never
//! approached however many people are on the couch.
//!
//! The refresh token is written to a `0600` file and **never** to the event
//! log, which is append-only and served unauthenticated on the LAN.

use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::path::Path;

use anyhow::{anyhow, bail, Context, Result};
use serde::{Deserialize, Serialize};

/// What SESH needs from the account: read what is playing, and change it.
pub const SCOPES: &[&str] = &["user-read-playback-state", "user-modify-playback-state"];

/// Where Spotify sends the browser back to.
///
/// Loopback, because Spotify requires HTTPS for redirect URIs *except* for
/// `http://127.0.0.1`. There is no certificate on a Pi in a living room, so
/// this is the only door available.
pub const CALLBACK_PORT: u16 = 7374;

/// Spotify's accounts host.
pub const ACCOUNTS: &str = "https://accounts.spotify.com";

/// The redirect URI to register in the Spotify dashboard.
pub fn redirect_uri() -> String {
    format!("http://127.0.0.1:{CALLBACK_PORT}/callback")
}

/// The URL a person opens to grant access.
pub fn authorize_url(
    accounts: &str,
    client_id: &str,
    redirect: &str,
    state: &str,
) -> Result<String> {
    let url = reqwest::Url::parse_with_params(
        &format!("{accounts}/authorize"),
        &[
            ("client_id", client_id),
            ("response_type", "code"),
            ("redirect_uri", redirect),
            ("scope", &SCOPES.join(" ")),
            ("state", state),
        ],
    )?;
    Ok(url.into())
}

/// Pull the authorisation code out of the callback's query string.
///
/// The `state` check is what stops a link someone else crafted from
/// completing *this* flow and binding the room to an account nobody here
/// chose.
pub fn code_from_query(query: &str, expected_state: &str) -> Result<String> {
    let url = reqwest::Url::parse(&format!("http://127.0.0.1/?{query}"))
        .context("the callback query was not parseable")?;

    let mut code = None;
    let mut state = None;
    let mut error = None;
    for (key, value) in url.query_pairs() {
        match key.as_ref() {
            "code" => code = Some(value.into_owned()),
            "state" => state = Some(value.into_owned()),
            "error" => error = Some(value.into_owned()),
            _ => {}
        }
    }

    if let Some(error) = error {
        bail!("Spotify refused the authorisation: {error}");
    }
    if state.as_deref() != Some(expected_state) {
        bail!("the callback's state did not match the one we sent; ignoring it");
    }
    code.ok_or_else(|| anyhow!("the callback carried no authorisation code"))
}

/// The path out of a raw HTTP request line, e.g. `GET /callback?code=x HTTP/1.1`.
pub fn target_from_request_line(line: &str) -> Result<String> {
    line.split_whitespace()
        .nth(1)
        .map(str::to_string)
        .ok_or_else(|| anyhow!("malformed HTTP request line: {line:?}"))
}

/// What is kept on disk between runs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoredTokens {
    /// The long-lived token used to mint access tokens.
    pub refresh_token: String,
}

/// Write the refresh token, readable only by its owner.
pub fn save_tokens(path: &Path, tokens: &StoredTokens) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, serde_json::to_string_pretty(tokens)?)
        .with_context(|| format!("writing {}", path.display()))?;
    restrict(path)?;
    Ok(())
}

/// Read the refresh token written by a previous authorisation.
pub fn load_tokens(path: &Path) -> Result<StoredTokens> {
    let text = std::fs::read_to_string(path).with_context(|| {
        format!(
            "reading {} — run `seshd auth-spotify` to authorise the house account",
            path.display()
        )
    })?;
    Ok(serde_json::from_str(&text)?)
}

#[cfg(unix)]
fn restrict(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    Ok(())
}

// SESH targets a Pi; this exists so the crate still builds elsewhere. A
// secret written without 0600 on such a machine is a real difference, so it
// says so rather than passing silently.
#[cfg(not(unix))]
fn restrict(path: &Path) -> Result<()> {
    tracing::warn!(
        path = %path.display(),
        "cannot restrict the token file's permissions on this platform"
    );
    Ok(())
}

/// A freshly minted access token.
#[derive(Debug, Clone)]
pub struct Access {
    /// The bearer token for API calls.
    pub token: String,
    /// Seconds until it expires.
    pub expires_in_s: i64,
    /// Spotify occasionally issues a replacement refresh token.
    pub refresh_token: Option<String>,
}

fn access_from_json(value: &serde_json::Value) -> Result<Access> {
    Ok(Access {
        token: value["access_token"]
            .as_str()
            .ok_or_else(|| anyhow!("token response carried no access_token"))?
            .to_string(),
        // Spotify documents an hour; default to that rather than treating a
        // missing field as "expires immediately" and refreshing every call.
        expires_in_s: value["expires_in"].as_i64().unwrap_or(3600),
        refresh_token: value["refresh_token"].as_str().map(str::to_string),
    })
}

/// Trade an authorisation code for tokens.
pub async fn exchange_code(
    http: &reqwest::Client,
    accounts: &str,
    client_id: &str,
    client_secret: &str,
    code: &str,
    redirect: &str,
) -> Result<(StoredTokens, Access)> {
    let value = post_token(
        http,
        accounts,
        client_id,
        client_secret,
        &[
            ("grant_type", "authorization_code"),
            ("code", code),
            ("redirect_uri", redirect),
        ],
    )
    .await?;

    let access = access_from_json(&value)?;
    let refresh_token = access
        .refresh_token
        .clone()
        .ok_or_else(|| anyhow!("Spotify returned no refresh token; cannot stay authorised"))?;

    Ok((StoredTokens { refresh_token }, access))
}

/// Mint a new access token from the stored refresh token.
pub async fn refresh_access(
    http: &reqwest::Client,
    accounts: &str,
    client_id: &str,
    client_secret: &str,
    refresh_token: &str,
) -> Result<Access> {
    let value = post_token(
        http,
        accounts,
        client_id,
        client_secret,
        &[
            ("grant_type", "refresh_token"),
            ("refresh_token", refresh_token),
        ],
    )
    .await?;
    access_from_json(&value)
}

async fn post_token(
    http: &reqwest::Client,
    accounts: &str,
    client_id: &str,
    client_secret: &str,
    form: &[(&str, &str)],
) -> Result<serde_json::Value> {
    let response = http
        .post(format!("{accounts}/api/token"))
        .basic_auth(client_id, Some(client_secret))
        .form(form)
        .send()
        .await
        .context("could not reach Spotify's accounts service")?;

    let status = response.status();
    let body = response.text().await.unwrap_or_default();
    if !status.is_success() {
        bail!("Spotify rejected the token request ({status}): {body}");
    }
    Ok(serde_json::from_str(&body)?)
}

/// Run the whole authorisation once, from a terminal.
///
/// Prints a URL, waits on the loopback callback, exchanges the code, and
/// writes the refresh token. Blocking rather than async on purpose: it is a
/// one-shot subcommand that runs instead of the daemon, not alongside it.
pub async fn run_flow(
    http: &reqwest::Client,
    accounts: &str,
    client_id: &str,
    client_secret: &str,
    token_path: &Path,
) -> Result<()> {
    let redirect = redirect_uri();
    let state = crate::join::new_token();
    let url = authorize_url(accounts, client_id, &redirect, &state)?;

    let listener = TcpListener::bind(("127.0.0.1", CALLBACK_PORT))
        .with_context(|| format!("binding 127.0.0.1:{CALLBACK_PORT} for the callback"))?;

    println!("\nAuthorise the house Spotify account by opening this URL:\n");
    println!("  {url}\n");
    println!("If you are working over SSH, forward the callback port first:");
    println!(
        "  ssh -L {CALLBACK_PORT}:127.0.0.1:{CALLBACK_PORT} {}@<pi>",
        whoami()
    );
    println!("\nMake sure {redirect} is registered as a redirect URI in the");
    println!("Spotify dashboard for this client id. Waiting for the callback...");

    let (stream, _) = listener
        .accept()
        .context("waiting for Spotify's callback")?;
    let target = read_target(&stream)?;
    let query = target.split_once('?').map(|(_, q)| q).unwrap_or_default();

    let outcome = code_from_query(query, &state);
    reply(&stream, &outcome);
    let code = outcome?;

    let (tokens, _) = exchange_code(http, accounts, client_id, client_secret, &code, &redirect)
        .await
        .context("exchanging the authorisation code")?;
    save_tokens(token_path, &tokens)?;

    println!(
        "\nAuthorised. Refresh token written to {}",
        token_path.display()
    );
    Ok(())
}

fn whoami() -> String {
    std::env::var("USER").unwrap_or_else(|_| "tate".into())
}

fn read_target(stream: &TcpStream) -> Result<String> {
    let mut line = String::new();
    BufReader::new(stream)
        .read_line(&mut line)
        .context("reading the callback request")?;
    target_from_request_line(&line)
}

/// Say something in the browser rather than leaving it on a blank page.
fn reply(mut stream: &TcpStream, outcome: &Result<String>) {
    let (status, message) = match outcome {
        Ok(_) => ("200 OK", "SESH is authorised. You can close this tab."),
        Err(error) => (
            "400 Bad Request",
            &*format!("Authorisation failed: {error}"),
        ),
    };
    let body = format!("<!doctype html><meta charset=utf-8><p>{message}</p>");
    let _ = write!(
        stream,
        "HTTP/1.1 {status}\r\nContent-Type: text/html; charset=utf-8\r\n\
         Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    const STATE: &str = "abc123";

    #[test]
    fn the_authorize_url_carries_everything_spotify_needs() {
        let url = authorize_url(ACCOUNTS, "client-1", &redirect_uri(), STATE).unwrap();
        let parsed = reqwest::Url::parse(&url).unwrap();
        let params: std::collections::HashMap<_, _> = parsed.query_pairs().collect();

        assert_eq!(parsed.host_str(), Some("accounts.spotify.com"));
        assert_eq!(parsed.path(), "/authorize");
        assert_eq!(params["client_id"].as_ref(), "client-1");
        assert_eq!(params["response_type"].as_ref(), "code");
        assert_eq!(params["redirect_uri"].as_ref(), redirect_uri());
        assert_eq!(params["state"].as_ref(), STATE);
        assert_eq!(
            params["scope"].as_ref(),
            "user-read-playback-state user-modify-playback-state"
        );
    }

    // The redirect must survive percent-encoding intact; a mangled one is
    // rejected by Spotify with a message that does not say why.
    #[test]
    fn the_redirect_uri_is_encoded_not_mangled() {
        let url = authorize_url(ACCOUNTS, "c", "http://127.0.0.1:7374/callback", STATE).unwrap();
        assert!(url.contains("redirect_uri=http%3A%2F%2F127.0.0.1%3A7374%2Fcallback"));
    }

    #[test]
    fn a_good_callback_yields_its_code() {
        assert_eq!(
            code_from_query(&format!("code=the-code&state={STATE}"), STATE).unwrap(),
            "the-code"
        );
    }

    #[test]
    fn a_refusal_is_reported_as_one() {
        let error = code_from_query(&format!("error=access_denied&state={STATE}"), STATE)
            .unwrap_err()
            .to_string();
        assert!(error.contains("access_denied"), "got {error}");
    }

    // Without this check, a link someone else crafted could complete this
    // flow and bind the room to an account nobody here chose.
    #[test]
    fn a_callback_with_the_wrong_state_is_refused() {
        assert!(code_from_query("code=the-code&state=somebody-elses", STATE).is_err());
        assert!(code_from_query("code=the-code", STATE).is_err());
    }

    #[test]
    fn a_callback_with_no_code_is_refused() {
        assert!(code_from_query(&format!("state={STATE}"), STATE).is_err());
    }

    #[test]
    fn the_request_target_is_pulled_off_the_request_line() {
        assert_eq!(
            target_from_request_line("GET /callback?code=x&state=y HTTP/1.1\r\n").unwrap(),
            "/callback?code=x&state=y"
        );
        assert!(target_from_request_line("").is_err());
    }

    #[test]
    fn tokens_round_trip_through_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested").join("spotify-token.json");
        let tokens = StoredTokens {
            refresh_token: "refresh-me".into(),
        };

        save_tokens(&path, &tokens).unwrap();
        assert_eq!(load_tokens(&path).unwrap(), tokens);
    }

    // The one file in SESH that holds a long-lived credential.
    #[cfg(unix)]
    #[test]
    fn the_token_file_is_readable_only_by_its_owner() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("spotify-token.json");
        save_tokens(
            &path,
            &StoredTokens {
                refresh_token: "refresh-me".into(),
            },
        )
        .unwrap();

        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600, "got {:o}", mode & 0o777);
    }

    #[test]
    fn a_missing_token_file_says_how_to_fix_it() {
        let error = load_tokens(Path::new("/nonexistent/spotify-token.json"))
            .unwrap_err()
            .to_string();
        assert!(error.contains("auth-spotify"), "got {error}");
    }

    #[test]
    fn an_access_response_is_parsed_with_a_sane_default_lifetime() {
        let access = access_from_json(&serde_json::json!({
            "access_token": "at", "expires_in": 3600, "refresh_token": "rt"
        }))
        .unwrap();
        assert_eq!(access.token, "at");
        assert_eq!(access.expires_in_s, 3600);
        assert_eq!(access.refresh_token.as_deref(), Some("rt"));

        // A refresh response usually omits both of the optional fields.
        let bare = access_from_json(&serde_json::json!({ "access_token": "at" })).unwrap();
        assert_eq!(bare.expires_in_s, 3600, "must not read as already expired");
        assert_eq!(bare.refresh_token, None);
    }

    #[test]
    fn a_token_response_with_no_access_token_is_an_error() {
        assert!(access_from_json(&serde_json::json!({ "error": "invalid_grant" })).is_err());
    }
}
