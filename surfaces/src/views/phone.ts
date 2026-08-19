/** The phone: search, queue, and vote things off. Used standing up, in a dark room. */

import type { Entry, Identity, MusicResponse, Track } from "../api";

/** Everything the phone screen renders from. */
export interface PhoneState {
  /** Who this phone is. */
  me: Identity | null;
  /** The queue as seshd last reported it, or null before the first load. */
  music: MusicResponse | null;
  /** Current search results. */
  results: Track[];
  /** What is in the search box. */
  query: string;
  /** True while a search is in flight. */
  searching: boolean;
  /** Something to tell the person, e.g. a failed add. */
  notice?: string | null;
}

function escapeHtml(value: string): string {
  return value
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(/"/g, "&quot;");
}

/** `3:42`, or empty when the length is unknown. */
export function duration(ms: number): string {
  if (!Number.isFinite(ms) || ms <= 0) return "";
  const total = Math.round(ms / 1000);
  const minutes = Math.floor(total / 60);
  const seconds = total % 60;
  return `${minutes}:${String(seconds).padStart(2, "0")}`;
}

function trackLine(title: string, artist: string): string {
  const name = title.trim() === "" ? "Unknown track" : title;
  return `<span class="track__title">${escapeHtml(name)}</span>
    <span class="track__artist">${escapeHtml(artist)}</span>`;
}

/** The now-playing block, or nothing at all when the room is quiet. */
function nowPlaying(music: MusicResponse, me: Identity | null): string {
  if (music.now_playing === null) {
    // A quiet room is a correct outcome, and saying so beats an empty box
    // that reads as a page that failed to load.
    return `<section class="now now--quiet"><p>Nothing playing</p></section>`;
  }
  const entry = music.now_playing;
  return `<section class="now">
    <p class="now__label">Now playing</p>
    <p class="track">${trackLine(entry.title, entry.artist)}</p>
    ${credit(entry)}
    ${vetoButton(entry, music.needed, me)}
  </section>`;
}

function credit(entry: Entry): string {
  if (entry.added_by === null) {
    // Started from the Spotify app rather than by anyone in the room. Saying
    // "added by nobody" would be worse than saying nothing.
    return "";
  }
  return `<p class="track__who">added by ${escapeHtml(entry.added_by)}</p>`;
}

/**
 * The veto button, showing the tally as `1/2`.
 *
 * Disabled once you have voted: one person, one vote is enforced by the
 * projection anyway, so a live button would be a lie about what a second tap
 * does.
 */
function vetoButton(entry: Entry, needed: number, me: Identity | null): string {
  const voted = me !== null && entry.vetoes.includes(me.id);
  const attributes = voted ? " disabled" : "";
  const classes = voted ? "veto veto--voted" : "veto";
  return `<button class="${classes}" data-veto="${entry.entry}"${attributes}>
    Veto <span class="veto__tally">${entry.vetoes.length}/${needed}</span>
  </button>`;
}

function pending(music: MusicResponse, me: Identity | null): string {
  if (music.pending.length === 0) {
    return `<p class="queue__empty">Nothing queued yet.</p>`;
  }
  const rows = music.pending
    .map(
      (entry) => `<li class="queue__row">
        <span class="track">${trackLine(entry.title, entry.artist)}</span>
        ${credit(entry)}
        ${vetoButton(entry, music.needed, me)}
      </li>`,
    )
    .join("");
  return `<ol class="queue">${rows}</ol>`;
}

function results(state: PhoneState): string {
  if (state.query.trim() === "") return "";
  if (state.searching) return `<p class="results__status">Searching…</p>`;
  if (state.results.length === 0) return `<p class="results__status">No matches.</p>`;

  const rows = state.results
    .map(
      (track, index) => `<li>
        <button class="result" data-result="${index}">
          <span class="track">${trackLine(track.title, track.artist)}</span>
          <span class="track__len">${duration(track.duration_ms)}</span>
        </button>
      </li>`,
    )
    .join("");
  return `<ul class="results">${rows}</ul>`;
}

/** Render the phone screen into `root`, replacing its contents. */
export function renderPhone(root: HTMLElement, state: PhoneState): void {
  if (state.music === null) {
    root.innerHTML = `<main class="phone"><p class="loading">Loading…</p></main>`;
    return;
  }

  // The source being down is worth saying out loud. Without it, tracks pile up
  // in a queue that plays nothing and the phone looks broken.
  const offline =
    state.music.player === "offline"
      ? `<p class="banner banner--warn">Music source offline — tracks will still queue.</p>`
      : "";

  const notice = state.notice
    ? `<p class="banner banner--error">${escapeHtml(state.notice)}</p>`
    : "";

  root.innerHTML = `<main class="phone">
    <header class="phone__head">
      <span class="wordmark">SESH</span>
      <span class="phone__me">${escapeHtml(state.me?.name ?? "")}</span>
    </header>
    ${offline}
    ${notice}
    ${nowPlaying(state.music, state.me)}
    <form class="search" autocomplete="off">
      <input class="search__input" name="q" type="search" placeholder="Search for a song"
        value="${escapeHtml(state.query)}" />
    </form>
    ${results(state)}
    <section class="queue__section">
      <h2 class="queue__title">Up next</h2>
      ${pending(state.music, state.me)}
    </section>
  </main>`;
}
