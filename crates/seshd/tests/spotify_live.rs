//! The Spotify client against the real API.
//!
//! Ignored by default: it needs the house account's credentials, a refresh
//! token from `seshd auth-spotify`, and the network. The stub in
//! `spotify_stub.rs` covers the failure paths deterministically; this covers
//! the one thing a stub cannot, which is whether Spotify still behaves the way
//! the client believes it does.
//!
//! Run it after any change to `player/spotify.rs`, and after Spotify announces
//! an API change:
//!
//! ```text
//! cargo test --test spotify_live -- --ignored --nocapture
//! ```
//!
//! It only reads and searches by default. Set `SESH_LIVE_MUTATE=1` to also
//! exercise `enqueue`/`skip`, which change what the house account is playing —
//! off by default so a routine test run cannot interrupt music in the room.

use std::path::PathBuf;

use seshd::player::spotify::{load_config, SpotifyPlayer, SEARCH_LIMIT};
use seshd::player::Player;

/// Where the credentials and token live on the Pi.
fn paths() -> Option<(PathBuf, PathBuf)> {
    let config = PathBuf::from(
        std::env::var("SESH_SPOTIFY_CONFIG").unwrap_or_else(|_| "/etc/sesh/spotify.toml".into()),
    );
    let token = std::env::var("SESH_SPOTIFY_TOKEN")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| ".".into()))
                .join(".local/share/sesh/spotify-token.json")
        });

    (config.exists() && token.exists()).then_some((config, token))
}

fn player() -> Option<SpotifyPlayer> {
    let (config, token) = paths()?;
    let config = load_config(&config).expect("reading the Spotify credentials");
    Some(SpotifyPlayer::new(config, token).expect("loading the stored refresh token"))
}

#[tokio::test]
#[ignore = "needs the house Spotify account and the network"]
async fn the_house_account_answers() {
    let Some(player) = player() else {
        panic!("no credentials or token on this machine; run `seshd auth-spotify` first");
    };

    // Search first: it is the only call that works with no active device, so
    // it is the cleanest proof that the refresh token and scopes are good.
    let results = player
        .search("bohemian rhapsody")
        .await
        .expect("search should succeed with a valid token");

    println!("\n--- search returned {} tracks", results.len());
    for track in &results {
        println!(
            "    {} — {} ({}ms)\n      {}",
            track.title, track.artist, track.duration_ms, track.uri
        );
    }

    assert!(!results.is_empty(), "search found nothing at all");
    assert!(
        results.len() <= SEARCH_LIMIT,
        "Spotify returned {} results, over the {SEARCH_LIMIT} cap",
        results.len()
    );

    // The mapping is the point of the module. A track SESH cannot name, show,
    // or time is one the queue cannot display or the conductor schedule.
    for track in &results {
        assert!(track.uri.starts_with("spotify:track:"), "bad uri {track:?}");
        assert!(!track.title.is_empty(), "no title on {track:?}");
        assert!(!track.artist.is_empty(), "no artist on {track:?}");
        assert!(track.duration_ms > 0, "no duration on {track:?}");
    }

    // Whatever the room is doing right now, including nothing. Both are
    // correct; what matters is that neither is an error.
    let playback = player.playback().await.expect("playback should not error");
    match &playback {
        Some(state) => println!(
            "\n--- playing: {} — {} at {}ms, is_playing={}, device={:?}",
            state.track.title,
            state.track.artist,
            state.progress_ms,
            state.is_playing,
            state.device
        ),
        None => println!("\n--- nothing is playing (204)"),
    }

    // Until librespot lands in Phase 6 there is no SESH device, and the
    // failure has to name that rather than surfacing a bare 404.
    match player.transfer().await {
        Ok(()) => println!("\n--- transfer succeeded; a SESH device exists"),
        Err(error) => {
            println!("\n--- transfer failed as expected: {error}");
            assert!(
                error.to_string().contains("SESH") || error.to_string().contains("device"),
                "transfer's error should name the missing device, got: {error}"
            );
        }
    }

    if std::env::var("SESH_LIVE_MUTATE").as_deref() != Ok("1") {
        println!("\n--- skipping enqueue/skip; set SESH_LIVE_MUTATE=1 to exercise them\n");
        return;
    }

    let track = &results[0];
    println!("\n--- enqueueing {}", track.uri);
    match player.enqueue(&track.uri).await {
        Ok(()) => println!("    enqueued"),
        Err(error) => println!("    enqueue failed: {error}"),
    }
    match player.skip().await {
        Ok(()) => println!("--- skipped"),
        Err(error) => println!("--- skip failed: {error}"),
    }
    println!();
}
