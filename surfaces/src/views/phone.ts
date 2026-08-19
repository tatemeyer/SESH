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

/** Render the phone screen into `root`.
 *
 * **The search box is built once and never rebuilt.** Every redraw used to
 * replace the whole subtree, and a phone redraws constantly: the 3s poll, plus
 * two draws per keystroke from the debounced search. Recreating the `<input>`
 * takes the caret with it and, on a phone, closes the keyboard — so a search
 * longer than one debounce was impossible to type.
 *
 * Refocusing afterwards is not a fix. A soft keyboard opens on a user gesture;
 * once the element it belonged to is gone, calling `focus()` on the replacement
 * does not bring it back on iOS. The element has to survive, so everything that
 * changes lives in sibling containers around it.
 */
export function renderPhone(root: HTMLElement, state: PhoneState): void {
  if (state.music === null) {
    root.innerHTML = `<main class="phone"><p class="loading">Loading\u2026</p></main>`;
    return;
  }

  const shell = ensureShell(root);

  // The source being down is worth saying out loud. Without it, tracks pile up
  // in a queue that plays nothing and the phone looks broken.
  const offline =
    state.music.player === "offline"
      ? `<p class="banner banner--warn">Music source offline \u2014 tracks will still queue.</p>`
      : "";

  const notice = state.notice
    ? `<p class="banner banner--error">${escapeHtml(state.notice)}</p>`
    : "";

  shell.me.textContent = state.me?.name ?? "";
  shell.banners.innerHTML = `${offline}${notice}`;
  shell.now.innerHTML = nowPlaying(state.music, state.me);
  shell.results.innerHTML = results(state);
  shell.queue.innerHTML = pending(state.music, state.me);

  // While the box has focus it owns its own value — writing to it would move
  // the caret to the end mid-word, which is the same bug in a quieter form.
  // Unfocused, state wins: that is the first render, and the controller
  // clearing `query` after a track is added.
  const typing = shell.search.ownerDocument.activeElement === shell.search;
  if (!typing && shell.search.value !== state.query) {
    shell.search.value = state.query;
  }
}

/** The parts of the phone screen that get rewritten, resolved once. */
interface Shell {
  me: HTMLElement;
  banners: HTMLElement;
  now: HTMLElement;
  search: HTMLInputElement;
  results: HTMLElement;
  queue: HTMLElement;
}

/** Build the phone's markup if it is not there yet, and return its parts. */
function ensureShell(root: HTMLElement): Shell {
  if (root.querySelector(".search__input") === null) {
    root.innerHTML = `<main class="phone">
    <header class="phone__head">
      <span class="wordmark">SESH</span>
      <span class="phone__me"></span>
    </header>
    <div class="phone__banners"></div>
    <div class="phone__now"></div>
    <form class="search" autocomplete="off">
      <input class="search__input" name="q" type="search" placeholder="Search for a song" />
    </form>
    <div class="phone__results"></div>
    <section class="queue__section">
      <h2 class="queue__title">Up next</h2>
      <div class="phone__queue"></div>
    </section>
  </main>`;
  }

  const pick = <T extends HTMLElement>(selector: string): T => {
    const found = root.querySelector<T>(selector);
    if (found === null) throw new Error(`phone shell is missing ${selector}`);
    return found;
  };

  return {
    me: pick(".phone__me"),
    banners: pick(".phone__banners"),
    now: pick(".phone__now"),
    search: pick<HTMLInputElement>(".search__input"),
    results: pick(".phone__results"),
    queue: pick(".phone__queue"),
  };
}
