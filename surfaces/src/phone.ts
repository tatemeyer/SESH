/** The phone surface: joining, then searching and voting. */

import {
  getMusic,
  heartbeat,
  joinRoom,
  queueTrack,
  searchTracks,
  vetoTrack,
  whoAmI,
  type Track,
} from "./api";
import { renderJoin, type JoinState } from "./views/join";
import { renderPhone, type PhoneState } from "./views/phone";

/** How long after the last keystroke to search. */
const SEARCH_DEBOUNCE_MS = 300;

/** How often to re-read the queue while the page is in front of someone. */
const POLL_MS = 3_000;

/** How often to say "still here". Well inside `presence::WINDOW_MS`. */
const HEARTBEAT_MS = 60_000;

/** Start the join screen. Called by `main.ts` for `/join`. */
export function startJoin(root: HTMLElement, search: string): void {
  const code = new URLSearchParams(search).get("c");
  const state: JoinState = { code, name: "", joining: false, notice: null };

  const draw = (): void => renderJoin(root, state);

  // An already-joined phone should not be asked its name again. Checked
  // before rendering anything, so the common case never flashes the form.
  void whoAmI().then((me) => {
    if (me !== null) {
      go("/phone");
      return;
    }
    draw();
  });

  root.addEventListener("input", (event) => {
    const input = event.target as HTMLInputElement;
    if (input.name === "name") state.name = input.value;
  });

  root.addEventListener("submit", (event) => {
    event.preventDefault();
    void attempt();
  });

  async function attempt(): Promise<void> {
    const name = state.name.trim();
    if (name === "" || state.joining || state.code === null) return;

    state.joining = true;
    state.notice = null;
    draw();

    try {
      await joinRoom(state.code, name);
      go("/phone");
    } catch (error) {
      console.error("sesh: join failed", error);
      // The overwhelmingly likely cause is a code that has rotated away,
      // which is a 60-second problem and not worth a technical message.
      state.notice = "That code has expired. Scan the one on the TV again.";
      state.joining = false;
      draw();
    }
  }
}

/** Start the queue screen. Called by `main.ts` for `/phone`. */
export function startPhone(root: HTMLElement): void {
  const state: PhoneState = {
    me: null,
    music: null,
    results: [],
    query: "",
    searching: false,
    notice: null,
  };

  const draw = (): void => renderPhone(root, state);
  let debounce: ReturnType<typeof setTimeout> | undefined;

  async function refresh(): Promise<void> {
    try {
      state.music = await getMusic();
      state.notice = null;
    } catch (error) {
      console.error("sesh: could not load the queue", error);
      state.notice = "Lost the room. Retrying…";
    }
    draw();
  }

  async function search(query: string): Promise<void> {
    if (query.trim() === "") {
      state.results = [];
      state.searching = false;
      draw();
      return;
    }
    state.searching = true;
    draw();
    try {
      state.results = await searchTracks(query);
    } catch (error) {
      console.error("sesh: search failed", error);
      state.results = [];
      state.notice = "Search is unavailable right now.";
    }
    state.searching = false;
    draw();
  }

  async function add(track: Track): Promise<void> {
    // Clear the search on the way out: the next thing anyone does is look at
    // what they just added, and leaving ten results on screen buries it.
    state.query = "";
    state.results = [];
    draw();
    try {
      await queueTrack(track);
    } catch (error) {
      console.error("sesh: queueing failed", error);
      state.notice = `Could not queue ${track.title}`;
    }
    await refresh();
  }

  async function veto(entry: number): Promise<void> {
    try {
      await vetoTrack(entry);
    } catch (error) {
      console.error("sesh: veto failed", error);
      state.notice = "Vote did not register.";
    }
    await refresh();
  }

  root.addEventListener("input", (event) => {
    const input = event.target as HTMLInputElement;
    if (input.name !== "q") return;
    state.query = input.value;
    // Debounced, because Spotify rate-limits and a phone keyboard produces a
    // request per letter otherwise.
    if (debounce !== undefined) clearTimeout(debounce);
    const query = input.value;
    debounce = setTimeout(() => void search(query), SEARCH_DEBOUNCE_MS);
  });

  // Enter on a phone keyboard submits rather than waiting out the debounce.
  root.addEventListener("submit", (event) => {
    event.preventDefault();
    if (debounce !== undefined) clearTimeout(debounce);
    void search(state.query);
  });

  root.addEventListener("click", (event) => {
    const target = event.target as HTMLElement;

    const result = target.closest("[data-result]");
    if (result) {
      const index = Number(result.getAttribute("data-result"));
      const track = state.results[index];
      if (track) void add(track);
      return;
    }

    const vetoTarget = target.closest("[data-veto]");
    if (vetoTarget) void veto(Number(vetoTarget.getAttribute("data-veto")));
  });

  void whoAmI().then((me) => {
    if (me === null) {
      // No token, or one seshd no longer knows. Either way the only way
      // forward is a fresh code off the TV.
      go("/join");
      return;
    }
    state.me = me;
    void refresh();
  });

  // Poll rather than subscribe: the WebSocket feed exists, but a phone in a
  // pocket with a dropped socket showing a stale queue is worse than three
  // seconds of lag, and `document.hidden` makes this nearly free.
  setInterval(() => {
    if (!document.hidden) void refresh();
  }, POLL_MS);

  // Presence, and therefore the veto denominator, is whoever has heartbeat
  // recently. A phone that is asleep in someone's pocket is not in the room.
  const beat = (): void => {
    if (!document.hidden) void heartbeat();
  };
  beat();
  setInterval(beat, HEARTBEAT_MS);
  document.addEventListener("visibilitychange", beat);
}

/** Navigate, replacing history so Back does not return to a spent code. */
function go(path: string): void {
  location.replace(path);
}
