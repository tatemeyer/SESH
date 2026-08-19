/** Typed client for seshd. Mirrors `crates/seshd/src/api`. */

/** One launchable app. Mirrors `AppSpec` in `config.rs`. */
export interface AppSpec {
  id: string;
  name: string;
  command: string;
  args: string[];
  icon: string;
}

/** Response of `GET /api/apps`. */
export interface AppsResponse {
  apps: AppSpec[];
  current: string | null;
}

/** One recorded event. Mirrors `Event` in `event.rs`. */
export interface SeshEvent {
  id: number;
  ts_ms: number;
  kind: string;
  actors: string[];
  subject: string | null;
  payload: unknown;
}

/** One track from a search. Mirrors `Track` in `player/mod.rs`. */
export interface Track {
  uri: string;
  title: string;
  artist: string;
  duration_ms: number;
}

/** One queue entry. Mirrors `Entry` in `projections/queue.rs`. */
export interface Entry extends Track {
  entry: number;
  added_by: string | null;
  vetoes: string[];
}

/** Response of `GET /api/music`. */
export interface MusicResponse {
  now_playing: Entry | null;
  pending: Entry[];
  present: string[];
  needed: number;
  /** `ok`, or `offline` when the music source is unreachable. */
  player: string;
}

/** Who this phone is. Mirrors `JoinResponse` in `api/join.rs`. */
export interface Identity {
  id: string;
  name: string;
}

/** Where the bearer token lives. Never in a URL, never in the log (D3). */
const TOKEN_KEY = "sesh.token";

/** The stored token, or null on a phone that has not joined. */
export function token(): string | null {
  try {
    return localStorage.getItem(TOKEN_KEY);
  } catch {
    // Safari in private mode throws on localStorage. A phone that cannot
    // remember its token can still join again; crashing the boot cannot.
    return null;
  }
}

/** Remember the token, or forget it when passed null. */
export function setToken(value: string | null): void {
  try {
    if (value === null) localStorage.removeItem(TOKEN_KEY);
    else localStorage.setItem(TOKEN_KEY, value);
  } catch {
    /* see `token` */
  }
}

function authorized(extra?: RequestInit): RequestInit {
  const bearer = token();
  return {
    ...extra,
    headers: {
      ...(extra?.headers ?? {}),
      ...(bearer ? { Authorization: `Bearer ${bearer}` } : {}),
    },
  };
}

async function ok(response: Response): Promise<Response> {
  if (!response.ok) {
    throw new Error(`${response.status} ${response.statusText}`);
  }
  return response;
}

/** Fetch the app registry and what is running. */
export async function listApps(fetchFn: typeof fetch = fetch): Promise<AppsResponse> {
  const response = await ok(await fetchFn("/api/apps"));
  return (await response.json()) as AppsResponse;
}

/** Launch an app by id. */
export async function launchApp(id: string, fetchFn: typeof fetch = fetch): Promise<void> {
  await ok(await fetchFn(`/api/apps/${id}/launch`, { method: "POST" }));
}

/** Quit whatever is running. */
export async function quitApp(fetchFn: typeof fetch = fetch): Promise<void> {
  await ok(await fetchFn("/api/apps/quit", { method: "POST" }));
}

/** How long to wait before rebuilding a dropped event socket. */
const RECONNECT_DELAY_MS = 1000;

/**
 * Subscribe to the live event feed. Returns a function that disconnects.
 * The socket URL is derived from the page so this works identically
 * against the Vite dev proxy and against seshd on the Pi.
 *
 * The socket reconnects itself: `seshd.service` sets `Restart=always`, so a
 * crash or an upgrade drops every surface's socket while Chromium stays up.
 * Without this the TV would show permanently stale state with no symptom.
 * `onReconnect` fires once each time a *replacement* socket opens — the
 * surface is level-triggered, so it re-fetches truth rather than replaying
 * the events it missed.
 */
export function connectEvents(
  onEvent: (event: SeshEvent) => void,
  WsCtor: typeof WebSocket = WebSocket,
  onReconnect?: () => void,
): () => void {
  const protocol = typeof location !== "undefined" && location.protocol === "https:" ? "wss" : "ws";
  const host = typeof location !== "undefined" ? location.host : "localhost:7373";
  const url = `${protocol}://${host}/ws`;

  let socket: WebSocket | null = null;
  let retry: ReturnType<typeof setTimeout> | undefined;
  let disconnected = false;

  function connect(isReconnect: boolean): void {
    const sock = new WsCtor(url);
    socket = sock;

    sock.onopen = () => {
      if (isReconnect) onReconnect?.();
    };

    sock.onmessage = (message: MessageEvent) => {
      try {
        onEvent(JSON.parse(message.data as string) as SeshEvent);
      } catch (error) {
        // A frame we cannot parse is not worth tearing the feed down for, but it
        // must not vanish silently — on the TV there is no console to inspect.
        console.error("sesh: unparseable event frame", error, message.data);
      }
    };

    // A failed socket fires error then close, so close alone drives the retry.
    sock.onerror = () => {
      console.error("sesh: event socket error");
    };

    sock.onclose = () => {
      if (disconnected) return;
      retry = setTimeout(() => connect(true), RECONNECT_DELAY_MS);
    };
  }

  connect(false);

  return () => {
    disconnected = true;
    if (retry !== undefined) clearTimeout(retry);
    socket?.close();
  };
}


/** Trade a scanned join code for a token, and keep it. */
export async function joinRoom(
  code: string,
  name: string,
  fetchFn: typeof fetch = fetch,
): Promise<Identity> {
  const response = await ok(
    await fetchFn("/api/join", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ code, name }),
    }),
  );
  const body = (await response.json()) as Identity & { token: string };
  setToken(body.token);
  return { id: body.id, name: body.name };
}

/**
 * Who this phone is, or null if its token is no longer good.
 *
 * Null rather than throwing: a stale token is the ordinary state of a phone
 * that joined last week, and it should land on the join screen rather than an
 * error.
 */
export async function whoAmI(fetchFn: typeof fetch = fetch): Promise<Identity | null> {
  if (token() === null) return null;
  const response = await fetchFn("/api/me", authorized());
  if (!response.ok) return null;
  return (await response.json()) as Identity;
}

/** Tell seshd this phone is still in the room. */
export async function heartbeat(fetchFn: typeof fetch = fetch): Promise<void> {
  await fetchFn("/api/heartbeat", authorized({ method: "POST" }));
}

/** The queue, who is here, and whether the source is answering. */
export async function getMusic(fetchFn: typeof fetch = fetch): Promise<MusicResponse> {
  const response = await ok(await fetchFn("/api/music"));
  return (await response.json()) as MusicResponse;
}

/** Search the music source. */
export async function searchTracks(
  query: string,
  fetchFn: typeof fetch = fetch,
): Promise<Track[]> {
  const response = await ok(
    await fetchFn(`/api/music/search?q=${encodeURIComponent(query)}`, authorized()),
  );
  return (await response.json()) as Track[];
}

/** Add a track to the queue. */
export async function queueTrack(track: Track, fetchFn: typeof fetch = fetch): Promise<void> {
  await ok(
    await fetchFn(
      "/api/music/queue",
      authorized({
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify(track),
      }),
    ),
  );
}

/** Vote to skip a queue entry. */
export async function vetoTrack(entry: number, fetchFn: typeof fetch = fetch): Promise<void> {
  await ok(
    await fetchFn(
      "/api/music/veto",
      authorized({
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ entry }),
      }),
    ),
  );
}
