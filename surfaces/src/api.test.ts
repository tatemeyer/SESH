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
});
