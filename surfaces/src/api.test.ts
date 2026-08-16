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
