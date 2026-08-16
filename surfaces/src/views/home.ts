/** The front door: a grid of launchable apps, navigable with a controller. */

import type { AppSpec } from "../api";

/** Everything the home screen renders from. */
export interface HomeState {
  apps: AppSpec[];
  current: string | null;
  selected: number;
  /** Replaces the hint line when something went wrong, e.g. a failed launch. */
  notice?: string | null;
}

function escapeHtml(value: string): string {
  return value
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(/"/g, "&quot;");
}

/** Render the home screen into `root`, replacing its contents. */
export function renderHome(root: HTMLElement, state: HomeState): void {
  if (state.apps.length === 0) {
    root.innerHTML = `<main class="home"><p class="empty">No apps configured. Check apps.toml.</p></main>`;
    return;
  }

  const tiles = state.apps
    .map((app, index) => {
      const classes = [
        "tile",
        index === state.selected ? "tile--selected" : "",
        app.id === state.current ? "tile--running" : "",
      ]
        .filter(Boolean)
        .join(" ");

      return `<button class="${classes}" data-app-id="${escapeHtml(app.id)}">
        <span class="tile__icon" data-icon="${escapeHtml(app.icon)}"></span>
        <span class="tile__name">${escapeHtml(app.name)}</span>
      </button>`;
    })
    .join("");

  // On the couch there is no console: without this, "that app isn't
  // installed" and "my button press didn't register" look identical.
  const hint = state.notice
    ? `<p class="hint hint--error">${escapeHtml(state.notice)}</p>`
    : state.current
      ? `<p class="hint">${escapeHtml(state.current)} is running — press B or Backspace to Quit</p>`
      : `<p class="hint">Select an app</p>`;

  root.innerHTML = `<main class="home"><h1 class="wordmark">SESH</h1><div class="grid">${tiles}</div>${hint}</main>`;
}

/** Tile columns. Kept here so `main.ts` and the CSS agree on one number. */
export const COLUMNS = 3;
