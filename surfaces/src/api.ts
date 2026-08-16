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
