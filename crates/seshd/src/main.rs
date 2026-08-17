//! Binary entry point: parse arguments, wire the daemon, serve.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, Result};
use clap::Parser;
use seshd::api::{router_with_ws, AppState};
use seshd::config::{detect_lan_ip, load_apps_file};
use seshd::join::JoinCodes;
use seshd::launcher::platform::ProcessPlatform;
use seshd::launcher::Launcher;
use seshd::presence::{self, Presence};
use seshd::reconcile::close_unfinished_launches;
use seshd::room::Room;
use seshd::store::{now_ms, Store};
use tower_http::services::{ServeDir, ServeFile};

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

    let store = Store::open(&args.db).with_context(|| format!("opening {}", args.db.display()))?;
    let room = Room::new(store)?;

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
    let presence = Arc::new(Presence::seeded(&room.roster(), now_ms()));
    tokio::spawn(presence::sweep_loop(presence.clone(), room.clone()));

    let join_base = join_base(&args);
    tracing::info!(%join_base, "phones will be sent here by the join QR");

    // Anything not under /api or /ws falls through to the surface bundle,
    // and unknown paths serve index.html so the surface owns its routing.
    let index = args.r#static.join("index.html");
    let surface = ServeDir::new(&args.r#static).not_found_service(ServeFile::new(index));

    let app = router_with_ws(AppState {
        room,
        launcher,
        join: Arc::new(JoinCodes::new()),
        presence,
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
