import { describe, expect, it, vi } from "vitest";
import { connectEvents, launchApp, listApps, quitApp } from "./api";

function jsonResponse(body: unknown): Response {
  return new Response(JSON.stringify(body), {
    status: 200,
    headers: { "content-type": "application/json" },
  });
}

describe("listApps", () => {
  it("requests the apps endpoint and returns the parsed body", async () => {
    const fetchFn = vi.fn().mockResolvedValue(
      jsonResponse({ apps: [{ id: "kodi", name: "Kodi", command: "kodi", args: [], icon: "movie" }], current: null }),
    );

    const result = await listApps(fetchFn as unknown as typeof fetch);

    expect(fetchFn).toHaveBeenCalledWith("/api/apps");
    expect(result.apps[0].id).toBe("kodi");
    expect(result.current).toBeNull();
  });

  it("throws when the server errors", async () => {
    const fetchFn = vi.fn().mockResolvedValue(new Response("nope", { status: 500 }));
    await expect(listApps(fetchFn as unknown as typeof fetch)).rejects.toThrow(/500/);
  });
});

describe("launchApp", () => {
  it("posts to the launch endpoint", async () => {
    const fetchFn = vi.fn().mockResolvedValue(new Response(null, { status: 204 }));
    await launchApp("kodi", fetchFn as unknown as typeof fetch);
    expect(fetchFn).toHaveBeenCalledWith("/api/apps/kodi/launch", { method: "POST" });
  });

  it("throws when the app is unknown", async () => {
    const fetchFn = vi.fn().mockResolvedValue(new Response(null, { status: 404 }));
    await expect(launchApp("n64", fetchFn as unknown as typeof fetch)).rejects.toThrow(/404/);
  });
});

describe("quitApp", () => {
  it("posts to the quit endpoint", async () => {
    const fetchFn = vi.fn().mockResolvedValue(new Response(null, { status: 204 }));
    await quitApp(fetchFn as unknown as typeof fetch);
    expect(fetchFn).toHaveBeenCalledWith("/api/apps/quit", { method: "POST" });
  });
});

/** Mirrors `RECONNECT_DELAY_MS` in `api.ts`, which is deliberately private. */
const RECONNECT_DELAY = 1000;
/** Mirrors `RECONNECT_MAX_DELAY_MS` in `api.ts`, likewise private. */
const RECONNECT_MAX_DELAY = 30_000;

type SocketHandlers = Record<"onopen" | "onmessage" | "onclose" | "onerror", (e?: unknown) => void>;

/**
 * A WebSocket stand-in that records one handler set per constructed socket,
 * so a test can drive close/open on each generation independently.
 */
function fakeSockets() {
  const handlers: SocketHandlers[] = [];
  const FakeWs = vi.fn(function (this: Record<string, unknown>) {
    const own = {} as SocketHandlers;
    handlers.push(own);
    this.close = vi.fn();
    for (const name of ["onopen", "onmessage", "onclose", "onerror"] as const) {
      Object.defineProperty(this, name, {
        set: (fn: (e?: unknown) => void) => {
          own[name] = fn;
        },
      });
    }
  });
  return { FakeWs, handlers };
}

describe("connectEvents", () => {
  it("parses incoming frames and hands them to the callback", () => {
    let onmessage: ((e: { data: string }) => void) | null = null;
    const close = vi.fn();
    const FakeWs = vi.fn(function (this: Record<string, unknown>) {
      this.close = close;
      Object.defineProperty(this, "onmessage", {
        set: (fn) => { onmessage = fn; },
      });
    });

    const received: unknown[] = [];
    const disconnect = connectEvents((e) => received.push(e), FakeWs as unknown as typeof WebSocket);

    onmessage!({ data: JSON.stringify({ id: 1, kind: "app.launched", subject: "kodi" }) });

    expect(received).toHaveLength(1);
    expect((received[0] as { kind: string }).kind).toBe("app.launched");

    disconnect();
    expect(close).toHaveBeenCalled();
  });

  it("reconnects after the socket closes and re-fetches on the new socket", () => {
    vi.useFakeTimers();
    const { FakeWs, handlers } = fakeSockets();
    const onReconnect = vi.fn();

    connectEvents(() => {}, FakeWs as unknown as typeof WebSocket, onReconnect);
    expect(FakeWs).toHaveBeenCalledTimes(1);

    // seshd restarted: Restart=always drops the socket while Chromium stays up.
    handlers[0].onclose();
    vi.advanceTimersByTime(RECONNECT_DELAY);
    expect(FakeWs).toHaveBeenCalledTimes(2);

    handlers[1].onopen();
    expect(onReconnect).toHaveBeenCalledTimes(1);

    vi.useRealTimers();
  });

  it("does not fire onReconnect for the very first connection", () => {
    const { FakeWs, handlers } = fakeSockets();
    const onReconnect = vi.fn();

    connectEvents(() => {}, FakeWs as unknown as typeof WebSocket, onReconnect);
    handlers[0].onopen();

    expect(onReconnect).not.toHaveBeenCalled();
  });

  it("disconnect cancels a retry that is already pending", () => {
    vi.useFakeTimers();
    const { FakeWs, handlers } = fakeSockets();

    const disconnect = connectEvents(() => {}, FakeWs as unknown as typeof WebSocket);
    handlers[0].onclose();
    disconnect();
    vi.advanceTimersByTime(RECONNECT_DELAY * 10);

    expect(FakeWs).toHaveBeenCalledTimes(1);
    vi.useRealTimers();
  });

  it("backs off instead of retrying on a flat interval", () => {
    vi.useFakeTimers();
    // Pin jitter so the schedule is deterministic; the jitter itself is
    // asserted separately below.
    vi.spyOn(Math, "random").mockReturnValue(0.5);
    const { FakeWs, handlers } = fakeSockets();

    connectEvents(() => {}, FakeWs as unknown as typeof WebSocket);

    // Third failure in a row must wait longer than the first did.
    handlers[0].onclose();
    vi.advanceTimersByTime(RECONNECT_DELAY);
    expect(FakeWs).toHaveBeenCalledTimes(2);

    handlers[1].onclose();
    vi.advanceTimersByTime(RECONNECT_DELAY);
    expect(FakeWs).toHaveBeenCalledTimes(2); // still waiting: the delay grew

    vi.advanceTimersByTime(RECONNECT_MAX_DELAY);
    expect(FakeWs).toHaveBeenCalledTimes(3);

    vi.restoreAllMocks();
    vi.useRealTimers();
  });

  it("caps the delay so a long outage does not back off forever", () => {
    vi.useFakeTimers();
    vi.spyOn(Math, "random").mockReturnValue(1);
    const { FakeWs, handlers } = fakeSockets();

    connectEvents(() => {}, FakeWs as unknown as typeof WebSocket);

    // A seshd that never comes back: twenty failures must not push the
    // delay past the cap, or the TV stops trying inside a human lifetime.
    for (let i = 0; i < 20; i += 1) {
      handlers[i].onclose();
      vi.advanceTimersByTime(RECONNECT_MAX_DELAY);
      expect(FakeWs).toHaveBeenCalledTimes(i + 2);
    }

    vi.restoreAllMocks();
    vi.useRealTimers();
  });

  it("jitters the delay so it is not the same every time", () => {
    vi.useFakeTimers();
    const { FakeWs, handlers } = fakeSockets();

    vi.spyOn(Math, "random").mockReturnValue(0);
    connectEvents(() => {}, FakeWs as unknown as typeof WebSocket);
    handlers[0].onclose();
    // With the lowest jitter the retry lands strictly before the full delay.
    vi.advanceTimersByTime(RECONNECT_DELAY - 1);
    expect(FakeWs).toHaveBeenCalledTimes(2);

    vi.restoreAllMocks();
    vi.useRealTimers();
  });

  it("resets the backoff once a socket opens again", () => {
    vi.useFakeTimers();
    vi.spyOn(Math, "random").mockReturnValue(0.5);
    const { FakeWs, handlers } = fakeSockets();

    connectEvents(() => {}, FakeWs as unknown as typeof WebSocket);

    handlers[0].onclose();
    vi.advanceTimersByTime(RECONNECT_MAX_DELAY);
    handlers[1].onclose();
    vi.advanceTimersByTime(RECONNECT_MAX_DELAY);
    expect(FakeWs).toHaveBeenCalledTimes(3);

    // A good socket clears the history, so the next drop is cheap again.
    handlers[2].onopen();
    handlers[2].onclose();
    vi.advanceTimersByTime(RECONNECT_DELAY);
    expect(FakeWs).toHaveBeenCalledTimes(4);

    vi.restoreAllMocks();
    vi.useRealTimers();
  });

  it("logs one error per outage rather than one per attempt", () => {
    vi.useFakeTimers();
    vi.spyOn(Math, "random").mockReturnValue(0.5);
    const error = vi.spyOn(console, "error").mockImplementation(() => {});
    const { FakeWs, handlers } = fakeSockets();

    connectEvents(() => {}, FakeWs as unknown as typeof WebSocket);

    // Chromium on the TV never restarts, so a seshd that stays down must not
    // grow the console without bound.
    for (let i = 0; i < 10; i += 1) {
      handlers[i].onerror();
      handlers[i].onclose();
      vi.advanceTimersByTime(RECONNECT_MAX_DELAY);
    }

    expect(error).toHaveBeenCalledTimes(1);

    vi.restoreAllMocks();
    vi.useRealTimers();
  });

  it("stops reconnecting once disconnected", () => {
    vi.useFakeTimers();
    const { FakeWs, handlers } = fakeSockets();

    const disconnect = connectEvents(() => {}, FakeWs as unknown as typeof WebSocket);
    disconnect();
    handlers[0].onclose();
    vi.advanceTimersByTime(RECONNECT_DELAY * 10);

    expect(FakeWs).toHaveBeenCalledTimes(1);
    vi.useRealTimers();
  });
});
