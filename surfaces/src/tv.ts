/** The TV surface: a grid of apps, a now-playing strip, and a join QR. */

import {
  connectEvents,
  getMusic,
  launchApp,
  listApps,
  quitApp,
  type SeshEvent,
} from "./api";
import { move, type Dir } from "./nav";
import { COLUMNS, renderHome, type HomeState } from "./views/home";

/** How often to re-fetch the join QR. Matches `JoinCodes::ROTATE_MS`. */
const QR_REFRESH_MS = 60_000;

/** Start the TV surface. Called by `main.ts` when the path is `/`. */
export function startTv(root: HTMLElement): void {
const state: HomeState = {
  apps: [],
  current: null,
  selected: 0,
  notice: null,
  music: null,
  qrNonce: 0,
};

function draw(): void {
  renderHome(root, state);
}

async function refresh(): Promise<void> {
  const { apps, current } = await listApps();
  state.apps = apps;
  state.current = current;
  state.selected = Math.min(state.selected, Math.max(0, apps.length - 1));
  await refreshMusic();
}

/**
 * Fetch the queue for the now-playing strip.
 *
 * Failure is swallowed on purpose. A box with no Spotify credentials is a
 * supported configuration, and the TV must still show its tiles — the
 * vision's rule is that every subsystem degrades to *the room still plays
 * media*.
 */
async function refreshMusic(): Promise<void> {
  try {
    state.music = await getMusic();
  } catch (error) {
    console.error("sesh: could not load the queue", error);
    state.music = null;
  }
  draw();
}

function navigate(dir: Dir): void {
  state.selected = move(state.selected, state.apps.length, COLUMNS, dir);
  draw();
}

/**
 * Launch, and put a failure on screen. `void launchApp(...)` alone turned a
 * missing binary into an unhandled rejection and rendered nothing.
 */
async function launch(id: string): Promise<void> {
  try {
    await launchApp(id);
    state.notice = null;
  } catch (error) {
    console.error("sesh: launch failed", error);
    state.notice = `Could not start ${id}`;
    draw();
  }
}

async function activate(): Promise<void> {
  const app = state.apps[state.selected];
  if (app) await launch(app.id);
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
  if (tile) void launch(tile.getAttribute("data-app-id")!);
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
  // Both `?.`s matter: without the second, a browser lacking getGamepads
  // throws here and never reaches requestAnimationFrame below, killing the
  // poll loop for good on a TV whose only input is the controller.
  const pad = navigator.getGamepads?.()?.find((p) => p !== null);
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
    } else if (event.kind.startsWith("music.")) {
      // The queue changes far more often than the app grid, and only the
      // strip depends on it, so this is a cheaper refresh than the full one.
      void refreshMusic();
    }
  },
  undefined,
  // A dropped socket means seshd restarted. The surface is level-triggered,
  // so re-fetch current state rather than replaying the missed events.
  () => void refresh(),
);

  // The join code rotates every 60s and the QR's URL never changes, so the
  // nonce is the only thing making the browser fetch the new one.
  setInterval(() => {
    state.qrNonce = (state.qrNonce ?? 0) + 1;
    draw();
  }, QR_REFRESH_MS);

  void refresh();
  requestAnimationFrame(pollGamepad);
}
