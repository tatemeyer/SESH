//! Binary entry point: parse arguments, wire the daemon, serve.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, Result};
use clap::Parser;
use seshd::api::{router_with_ws, surface_service, AppState};
use seshd::clock::{Clock, SystemClock};
use seshd::conductor::{self, Conductor, Status};
use seshd::config::{detect_lan_ip, load_apps_file};
use seshd::join::JoinCodes;
use seshd::launcher::platform::ProcessPlatform;
use seshd::launcher::Launcher;
use seshd::player::auth;
use seshd::player::spotify::{load_config as load_spotify_config, SpotifyPlayer};
use seshd::player::Player;
use seshd::presence::{self, Presence};
use seshd::reconcile::close_unfinished_launches;
use seshd::room::Room;
use seshd::store::Store;

/// The SESH room daemon.
#[derive(Debug, Parser)]
#[command(name = "seshd", version)]
struct Args {
    /// Path to the event log database. Created if absent.
    #[arg(long, default_value = "sesh.db")]
    db: PathBuf,

    /// Path to the app registry.
    #[arg(long, default_value = "deploy/apps.toml")]
    apps: PathBuf,

    /// Directory holding the built surface bundle.
    #[arg(long, default_value = "surfaces/dist")]
    r#static: PathBuf,

    /// Address to bind.
    #[arg(long, default_value = "0.0.0.0:7373")]
    bind: String,

    /// Origin the join QR points phones at, e.g. `http://192.168.1.20:7373`.
    ///
    /// Detected from the routing table when omitted. Set it when the Pi has
    /// several interfaces and the detected one is not the one guests are on.
    #[arg(long)]
    advertise_url: Option<String>,

    /// Spotify credentials for the house account.
    ///
    /// `global` so it can be written after the subcommand, which is how
    /// anyone would type `seshd auth-spotify --spotify-config ...`.
    #[arg(long, global = true, default_value = "/etc/sesh/spotify.toml")]
    spotify_config: PathBuf,

    /// Where the Spotify refresh token is kept. Written `0600`.
    #[arg(long, global = true)]
    spotify_token: Option<PathBuf>,

    #[command(subcommand)]
    command: Option<Command>,
}

/// Things `seshd` can do instead of running the room.
#[derive(Debug, clap::Subcommand)]
enum Command {
    /// Authorise the house Spotify account. Run once, by hand.
    AuthSpotify,
}

impl Args {
    /// Where the refresh token lives.
    ///
    /// Under the user's data directory beside the event log, not in `/etc`:
    /// it is written by the daemon at runtime when Spotify rotates it, and a
    /// root-owned file would make that fail at the worst moment.
    fn spotify_token_path(&self) -> PathBuf {
        self.spotify_token.clone().unwrap_or_else(|| {
            let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
            PathBuf::from(home).join(".local/share/sesh/spotify-token.json")
        })
    }
}

/// Run the Spotify authorisation flow once, from a terminal.
async fn authorise_spotify(args: &Args) -> Result<()> {
    let config = load_spotify_config(&args.spotify_config).with_context(|| {
        format!(
            "reading {}. Copy deploy/spotify.toml there and fill in the \
             client id and secret from the Spotify dashboard.",
            args.spotify_config.display()
        )
    })?;

    auth::run_flow(
        &reqwest::Client::new(),
        auth::ACCOUNTS,
        &config.client_id,
        &config.client_secret,
        &args.spotify_token_path(),
    )
    .await
}

/// Build the music source, if this box has been given credentials.
///
/// Absence is not an error. A Pi with no Spotify app still launches Kodi and
/// still keeps a queue; the vision's rule is that every subsystem degrades to
/// *the room still plays media*, and refusing to boot over a missing music
/// token would break that at the first hurdle.
fn build_player(args: &Args, clock: Arc<dyn Clock>) -> Option<Arc<dyn Player>> {
    let token_path = args.spotify_token_path();
    if !args.spotify_config.exists() || !token_path.exists() {
        tracing::info!(
            config = %args.spotify_config.display(),
            token = %token_path.display(),
            "no Spotify credentials; the queue will accept tracks but nothing will play"
        );
        return None;
    }

    match load_spotify_config(&args.spotify_config)
        .and_then(|config| SpotifyPlayer::new(config, token_path))
        .map(|player| player.with_clock(clock))
    {
        Ok(player) => Some(Arc::new(player)),
        Err(error) => {
            // Warn rather than exit, for the same reason as above.
            tracing::warn!(%error, "could not build the Spotify player; music is disabled");
            None
        }
    }
}

/// Work out what to put in the join QR.
///
/// Not derivable from the request: the TV loads the QR over `127.0.0.1`, so a
/// QR built from the `Host` header would send every phone to its own loopback.
fn join_base(args: &Args) -> String {
    if let Some(url) = &args.advertise_url {
        return url.trim_end_matches('/').to_string();
    }

    let port = args
        .bind
        .parse::<SocketAddr>()
        .map(|addr| addr.port())
        .unwrap_or(7373);

    match detect_lan_ip() {
        Some(ip) => format!("http://{ip}:{port}"),
        None => {
            tracing::warn!(
                "no LAN address found; the join QR will point at loopback and \
                 no phone will be able to use it. Pass --advertise-url."
            );
            format!("http://127.0.0.1:{port}")
        }
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "seshd=info,tower_http=warn".into()),
        )
        .init();

    let args = Args::parse();

    // A one-shot subcommand runs *instead of* the room, not alongside it: it
    // needs a terminal to print a URL to and a human to open it.
    if let Some(Command::AuthSpotify) = args.command {
        return authorise_spotify(&args).await;
    }

    let clock: Arc<dyn Clock> = Arc::new(SystemClock::new());

    let store = Store::open(&args.db)
        .with_context(|| format!("opening {}", args.db.display()))?
        .with_clock(clock.clone());
    let room = Room::new(store)?;

    // Nothing here waits for the clock, deliberately — see `clock`'s module
    // doc. Reconciliation runs against whatever the clock says and its row is
    // marked when that could not be trusted.

    // Before anything can read the log, close launches a previous run left
    // open. Restarting `seshd` kills the apps it launched, so their exits were
    // never observed and never recorded — leaving a log that claims they are
    // still running. Do this before binding so no client sees that state.
    let closed = close_unfinished_launches(&room)?;
    if !closed.is_empty() {
        tracing::warn!(
            apps = ?closed,
            "closed launches left open by a previous run — the last shutdown was not clean"
        );
    }

    let apps = load_apps_file(&args.apps)?;
    tracing::info!(count = apps.len(), "loaded app registry");

    let launcher = Launcher::new(apps, Arc::new(ProcessPlatform::new()), room.clone());
    tokio::spawn(Launcher::reap_loop(launcher.clone()));

    // Seed presence from the rebuilt roster. The log may already say people
    // are here, and without this their first heartbeat after a restart would
    // announce an arrival for someone who never left — one spurious row in the
    // log per person per restart, forever.
    let presence = Arc::new(Presence::seeded(&room.roster(), clock.mono_ms()));
    tokio::spawn(presence::sweep_loop(
        presence.clone(),
        room.clone(),
        clock.clone(),
    ));

    // Music, if this box has been given the credentials for it.
    let music = Arc::new(Status::new());
    let player = build_player(&args, clock.clone());
    if let Some(player) = player.clone() {
        let conductor = Conductor::new(room.clone(), player, music.clone());

        // D8: reconcile against the *source*, not the log. librespot lives
        // outside seshd's cgroup, so after a restart the music is still
        // playing and it is the log that is out of date — the opposite of the
        // launch reconciliation above. One pass before binding, so no client
        // ever sees the stale picture.
        let wait = conductor.tick().await;
        tracing::info!(
            ?wait,
            online = music.is_online(),
            "reconciled with the music source"
        );

        tokio::spawn(conductor::run_loop(conductor));
    }

    let join_base = join_base(&args);
    tracing::info!(%join_base, "phones will be sent here by the join QR");

    // Anything not under /api or /ws falls through to the surface bundle,
    // and unknown paths serve index.html so the surface owns its routing.
    let surface = surface_service(&args.r#static);

    let app = router_with_ws(AppState {
        room,
        launcher,
        join: Arc::new(JoinCodes::new()),
        presence,
        player,
        music,
        clock: clock.clone(),
        join_base,
    })
    .fallback_service(surface);

    let listener = tokio::net::TcpListener::bind(&args.bind)
        .await
        .with_context(|| format!("binding {}", args.bind))?;
    tracing::info!(addr = %listener.local_addr()?, "seshd listening");

    axum::serve(listener, app).await?;
    Ok(())
}
