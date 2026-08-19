import { describe, expect, it } from "vitest";
import { renderHome, type HomeState } from "./home";
import type { AppSpec, Entry, MusicResponse } from "../api";

const APPS: AppSpec[] = [
  { id: "kodi", name: "Kodi", command: "kodi", args: [], icon: "movie" },
  { id: "retroarch", name: "RetroArch", command: "retroarch", args: [], icon: "gamepad" },
];

function root(): HTMLElement {
  return document.createElement("div");
}

describe("renderHome", () => {
  it("renders a tile per app", () => {
    const el = root();
    renderHome(el, { apps: APPS, current: null, selected: 0 });

    const tiles = el.querySelectorAll("[data-app-id]");
    expect(tiles).toHaveLength(2);
    expect(tiles[0].getAttribute("data-app-id")).toBe("kodi");
    expect(tiles[0].textContent).toContain("Kodi");
  });

  it("marks exactly one tile selected", () => {
    const el = root();
    renderHome(el, { apps: APPS, current: null, selected: 1 });

    const selected = el.querySelectorAll(".tile--selected");
    expect(selected).toHaveLength(1);
    expect(selected[0].getAttribute("data-app-id")).toBe("retroarch");
  });

  it("marks the running app", () => {
    const el = root();
    renderHome(el, { apps: APPS, current: "kodi", selected: 0 });

    const running = el.querySelector(".tile--running");
    expect(running?.getAttribute("data-app-id")).toBe("kodi");
  });

  it("shows a quit hint only while something is running", () => {
    const idle = root();
    renderHome(idle, { apps: APPS, current: null, selected: 0 });
    expect(idle.textContent).not.toContain("Quit");

    const busy = root();
    renderHome(busy, { apps: APPS, current: "kodi", selected: 0 });
    expect(busy.textContent).toContain("Quit");
  });

  it("replaces the hint with a notice when a launch fails", () => {
    const el = root();
    renderHome(el, {
      apps: APPS,
      current: "kodi",
      selected: 0,
      notice: "Could not start moonlight",
    });

    expect(el.textContent).toContain("Could not start moonlight");
    expect(el.textContent).not.toContain("Quit");
    expect(el.querySelector(".hint--error")).not.toBeNull();
  });

  it("shows a message when the registry is empty", () => {
    const el = root();
    renderHome(el, { apps: [], current: null, selected: 0 });
    expect(el.textContent).toContain("No apps configured");
  });

  // This used to assert `querySelector("img")` was null, which worked only
  // while the home screen contained no images of its own. The join QR is a
  // legitimate one, so the check now names the injected element instead —
  // stricter than before, since it also proves the name survives as text
  // rather than being silently dropped.
  it("escapes app names so a registry cannot inject markup", () => {
    const el = root();
    renderHome(el, {
      apps: [{ id: "x", name: "<img src=x onerror=alert(1)>", command: "x", args: [], icon: "" }],
      current: null,
      selected: 0,
    });
    expect(el.querySelector("img[onerror]")).toBeNull();
    expect(el.querySelector('img[src="x"]')).toBeNull();
    expect(el.textContent).toContain("<img src=x onerror=alert(1)>");
  });
});


describe("renderHome now-playing strip", () => {
  function track(overrides: Partial<Entry> = {}): Entry {
    return {
      entry: 1,
      uri: "spotify:track:a",
      title: "Teenage Dirtbag",
      artist: "Wheatus",
      duration_ms: 240_000,
      added_by: "sam",
      vetoes: [],
      ...overrides,
    };
  }

  function withMusic(overrides: Partial<MusicResponse>): HomeState {
    return {
      apps: APPS,
      current: null,
      selected: 0,
      music: {
        now_playing: null,
        pending: [],
        present: [],
        needed: 2,
        player: "ok",
        ...overrides,
      },
    };
  }

  // A room with no music must look exactly as it did before Arc 2, rather
  // than growing a permanent empty shelf under the grid.
  it("shows no strip when nothing is playing", () => {
    const el = root();
    renderHome(el, withMusic({ now_playing: null }));
    expect(el.querySelector(".nowbar")).toBeNull();
  });

  it("shows no strip when there is no queue at all", () => {
    const el = root();
    renderHome(el, { apps: APPS, current: null, selected: 0 });
    expect(el.querySelector(".nowbar")).toBeNull();
  });

  it("shows the track, the artist, and who queued it", () => {
    const el = root();
    renderHome(el, withMusic({ now_playing: track() }));

    const bar = el.querySelector(".nowbar")!;
    expect(bar.textContent).toContain("Teenage Dirtbag");
    expect(bar.textContent).toContain("Wheatus");
    expect(bar.textContent).toContain("added by sam");
  });

  it("credits no one for a track the room did not queue", () => {
    const el = root();
    renderHome(el, withMusic({ now_playing: track({ added_by: null }) }));
    expect(el.querySelector(".nowbar")!.textContent).not.toContain("added by");
  });

  it("counts how many are waiting", () => {
    const el = root();
    renderHome(el, withMusic({ now_playing: track(), pending: [track(), track()] }));
    expect(el.querySelector(".nowbar")!.textContent).toContain("2 waiting");

    renderHome(el, withMusic({ now_playing: track(), pending: [] }));
    expect(el.querySelector(".nowbar")!.textContent).not.toContain("waiting");
  });

  it("escapes track metadata so a title cannot inject markup", () => {
    const el = root();
    renderHome(el, withMusic({ now_playing: track({ title: "<img src=x onerror=alert(1)>" }) }));
    expect(el.querySelector("img[onerror]")).toBeNull();
  });
});

describe("the join QR", () => {
  it("is on the home screen with a label", () => {
    const el = root();
    renderHome(el, { apps: APPS, current: null, selected: 0 });

    expect(el.querySelector(".joinqr__img")).not.toBeNull();
    expect(el.textContent).toContain("Scan to join");
  });

  // The code rotates every 60s and the URL never changes, so without the
  // nonce the TV would keep showing a code that stopped working a minute
  // after it appeared.
  it("changes its URL when the nonce is bumped", () => {
    const el = root();
    renderHome(el, { apps: APPS, current: null, selected: 0, qrNonce: 1 });
    const first = el.querySelector("img.joinqr__img")!.getAttribute("src");

    renderHome(el, { apps: APPS, current: null, selected: 0, qrNonce: 2 });
    const second = el.querySelector("img.joinqr__img")!.getAttribute("src");

    expect(first).not.toBe(second);
  });
});
