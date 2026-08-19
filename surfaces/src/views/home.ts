/** The front door: a grid of launchable apps, navigable with a controller. */

import type { AppSpec, MusicResponse } from "../api";

/** Everything the home screen renders from. */
export interface HomeState {
  apps: AppSpec[];
  current: string | null;
  selected: number;
  /** Replaces the hint line when something went wrong, e.g. a failed launch. */
  notice?: string | null;
  /** The queue, when there is one. Null before the first load. */
  music?: MusicResponse | null;
  /**
   * Bumped to force the browser to re-fetch the join QR.
   *
   * The code rotates every 60s and the URL never changes, so without a
   * changing query string the TV would keep showing a photograph of a code
   * that stopped working a minute after it appeared.
   */
  qrNonce?: number;
}

function escapeHtml(value: string): string {
  return value
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(/"/g, "&quot;");
}

/**
 * The strip under the grid: what is playing and how much is waiting.
 *
 * Empty string when nothing is playing, so a room with no music looks exactly
 * as it did before Arc 2 rather than growing a permanent empty shelf.
 */
function nowPlayingCard(music: MusicResponse | null | undefined): string {
  if (!music || music.now_playing === null) return "";

  const entry = music.now_playing;
  const who = entry.added_by === null ? "" : ` · added by ${escapeHtml(entry.added_by)}`;
  const waiting =
    music.pending.length === 0
      ? ""
      : ` · ${music.pending.length} waiting`;

  return `<section class="nowbar">
    <span class="nowbar__icon" aria-hidden="true"></span>
    <span class="nowbar__track">
      <span class="nowbar__title">${escapeHtml(entry.title)}</span>
      <span class="nowbar__artist">${escapeHtml(entry.artist)}</span>
    </span>
    <span class="nowbar__meta">${who}${waiting}</span>
  </section>`;
}

/** The join QR, small, in a corner. */
function joinQr(nonce: number | undefined): string {
  return `<aside class="joinqr">
    <img class="joinqr__img" src="/api/join/qr.svg?t=${nonce ?? 0}" alt="" />
    <span class="joinqr__label">Scan to join</span>
  </aside>`;
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

  root.innerHTML = `<main class="home"><h1 class="wordmark">SESH</h1><div class="grid">${tiles}</div>${nowPlayingCard(state.music)}${hint}${joinQr(state.qrNonce)}</main>`;
}

/** Tile columns. Kept here so `main.ts` and the CSS agree on one number. */
export const COLUMNS = 3;
