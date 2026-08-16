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

/**
 * Subscribe to the live event feed. Returns a function that disconnects.
 * The socket URL is derived from the page so this works identically
 * against the Vite dev proxy and against seshd on the Pi.
 */
export function connectEvents(
  onEvent: (event: SeshEvent) => void,
  WsCtor: typeof WebSocket = WebSocket,
): () => void {
  const protocol = typeof location !== "undefined" && location.protocol === "https:" ? "wss" : "ws";
  const host = typeof location !== "undefined" ? location.host : "localhost:7373";
  const socket = new WsCtor(`${protocol}://${host}/ws`);

  socket.onmessage = (message: MessageEvent) => {
    try {
      onEvent(JSON.parse(message.data as string) as SeshEvent);
    } catch {
      // A frame we cannot parse is not worth tearing the feed down for.
    }
  };

  return () => socket.close();
}
