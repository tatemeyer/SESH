/** Bootstrap: load state, render, and drive selection from a controller. */

import "./styles.css";
import { connectEvents, launchApp, listApps, quitApp, type SeshEvent } from "./api";
import { move, type Dir } from "./nav";
import { COLUMNS, renderHome, type HomeState } from "./views/home";

const root = document.getElementById("app")!;

const state: HomeState = { apps: [], current: null, selected: 0 };

function draw(): void {
  renderHome(root, state);
}

async function refresh(): Promise<void> {
  const { apps, current } = await listApps();
  state.apps = apps;
  state.current = current;
  state.selected = Math.min(state.selected, Math.max(0, apps.length - 1));
  draw();
}

function navigate(dir: Dir): void {
  state.selected = move(state.selected, state.apps.length, COLUMNS, dir);
  draw();
}

async function activate(): Promise<void> {
  const app = state.apps[state.selected];
  if (app) await launchApp(app.id);
}

const KEYS: Record<string, () => void | Promise<void>> = {
  ArrowUp: () => navigate("up"),
  ArrowDown: () => navigate("down"),
  ArrowLeft: () => navigate("left"),
  ArrowRight: () => navigate("right"),
  Enter: activate,
  Backspace: quitApp,
};

window.addEventListener("keydown", (e) => {
  // Without this, holding Enter fires `activate` at the OS key-repeat rate,
  // pushing overlapping launches at seshd for as long as the key is down.
  if (e.repeat) return;
  const handler = KEYS[e.key];
  if (handler) {
    e.preventDefault();
    void handler();
  }
});

root.addEventListener("click", (e) => {
  const tile = (e.target as HTMLElement).closest("[data-app-id]");
  if (tile) void launchApp(tile.getAttribute("data-app-id")!);
});

// Gamepad: the Gamepad API has no event for button presses, so it must be
// polled. Edge-detect against the previous frame so a held button fires once.
const GAMEPAD_ACTIONS: Array<[number, () => void | Promise<void>]> = [
  [12, () => navigate("up")],
  [13, () => navigate("down")],
  [14, () => navigate("left")],
  [15, () => navigate("right")],
  [0, activate],
  [1, quitApp],
];

let previous: boolean[] = [];

function pollGamepad(): void {
  const pad = navigator.getGamepads?.().find((p) => p !== null);
  if (pad) {
    for (const [button, action] of GAMEPAD_ACTIONS) {
      const pressed = pad.buttons[button]?.pressed ?? false;
      if (pressed && !previous[button]) void action();
      previous[button] = pressed;
    }
  }
  requestAnimationFrame(pollGamepad);
}

connectEvents(
  (event: SeshEvent) => {
    if (event.kind === "app.launched" || event.kind === "app.exited") {
      void refresh();
    }
  },
  undefined,
  // A dropped socket means seshd restarted. The surface is level-triggered,
  // so re-fetch current state rather than replaying the missed events.
  () => void refresh(),
);

void refresh();
requestAnimationFrame(pollGamepad);
