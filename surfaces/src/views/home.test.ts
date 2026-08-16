import { describe, expect, it } from "vitest";
import { renderHome } from "./home";
import type { AppSpec } from "../api";

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

  it("escapes app names so a registry cannot inject markup", () => {
    const el = root();
    renderHome(el, {
      apps: [{ id: "x", name: "<img src=x onerror=alert(1)>", command: "x", args: [], icon: "" }],
      current: null,
      selected: 0,
    });
    expect(el.querySelector("img")).toBeNull();
  });
});
