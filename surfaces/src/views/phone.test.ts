import { describe, expect, it } from "vitest";
import { duration, renderPhone, type PhoneState } from "./phone";
import type { Entry, Identity, MusicResponse } from "../api";

function root(): HTMLElement {
  const el = document.createElement("div");
  document.body.appendChild(el);
  return el;
}

const SAM: Identity = { id: "sam", name: "Sam" };

function entry(overrides: Partial<Entry> = {}): Entry {
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

function music(overrides: Partial<MusicResponse> = {}): MusicResponse {
  return {
    now_playing: null,
    pending: [],
    present: ["sam", "marcus"],
    needed: 2,
    player: "ok",
    ...overrides,
  };
}

function state(overrides: Partial<PhoneState> = {}): PhoneState {
  return {
    me: SAM,
    music: music(),
    results: [],
    query: "",
    searching: false,
    notice: null,
    ...overrides,
  };
}

describe("duration", () => {
  it("formats minutes and seconds", () => {
    expect(duration(240_000)).toBe("4:00");
    expect(duration(212_000)).toBe("3:32");
    expect(duration(9_000)).toBe("0:09");
  });

  // A queue entry with no known length is normal — the log keeps whatever the
  // phone sent — and "0:00" would read as a broken track.
  it("says nothing when the length is unknown", () => {
    expect(duration(0)).toBe("");
    expect(duration(-1)).toBe("");
    expect(duration(NaN)).toBe("");
  });
});

describe("renderPhone", () => {
  it("says the room is quiet rather than showing an empty box", () => {
    const el = root();
    renderPhone(el, state());
    expect(el.textContent).toContain("Nothing playing");
    expect(el.textContent).toContain("Nothing queued yet");
  });

  it("shows what is playing and who put it on", () => {
    const el = root();
    renderPhone(el, state({ music: music({ now_playing: entry() }) }));

    expect(el.textContent).toContain("Teenage Dirtbag");
    expect(el.textContent).toContain("Wheatus");
    expect(el.textContent).toContain("added by sam");
  });

  // A track someone started in the Spotify app has no one to credit, and
  // "added by nobody" is worse than saying nothing.
  it("credits no one for a track the room did not queue", () => {
    const el = root();
    renderPhone(el, state({ music: music({ now_playing: entry({ added_by: null }) }) }));
    expect(el.textContent).not.toContain("added by");
  });

  it("lists the queue in order with a veto tally on each", () => {
    const el = root();
    renderPhone(
      el,
      state({
        music: music({
          pending: [
            entry({ entry: 1, title: "First" }),
            entry({ entry: 2, title: "Second", vetoes: ["marcus"] }),
          ],
        }),
      }),
    );

    const rows = el.querySelectorAll(".queue__row");
    expect(rows.length).toBe(2);
    expect(rows[0].textContent).toContain("First");
    expect(rows[1].textContent).toContain("Second");

    const tallies = [...el.querySelectorAll(".veto__tally")].map((n) => n.textContent?.trim());
    expect(tallies).toEqual(["0/2", "1/2"]);
  });

  it("disables the veto button once you have voted", () => {
    const el = root();
    renderPhone(
      el,
      state({ music: music({ pending: [entry({ entry: 7, vetoes: ["sam"] })] }) }),
    );

    const button = el.querySelector<HTMLButtonElement>('[data-veto="7"]')!;
    expect(button.disabled).toBe(true);
  });

  // The discriminating half: somebody *else* voting must leave your button
  // live. Asserting only the case above would pass against an implementation
  // that disabled the button whenever the tally was non-zero.
  it("leaves the veto button live when someone else voted", () => {
    const el = root();
    renderPhone(
      el,
      state({ music: music({ pending: [entry({ entry: 7, vetoes: ["marcus"] })] }) }),
    );

    const button = el.querySelector<HTMLButtonElement>('[data-veto="7"]')!;
    expect(button.disabled).toBe(false);
    expect(button.textContent).toContain("1/2");
  });

  it("says when the music source is offline", () => {
    const el = root();
    renderPhone(el, state({ music: music({ player: "offline" }) }));
    expect(el.textContent).toContain("offline");
    expect(el.textContent).toContain("still queue");
  });

  it("shows search results only once there is a query", () => {
    const el = root();
    const results = [
      { uri: "spotify:track:a", title: "Found It", artist: "Someone", duration_ms: 200_000 },
    ];

    renderPhone(el, state({ results, query: "" }));
    expect(el.querySelectorAll(".result").length).toBe(0);

    renderPhone(el, state({ results, query: "found" }));
    expect(el.querySelectorAll(".result").length).toBe(1);
    expect(el.textContent).toContain("Found It");
  });

  it("distinguishes a search in flight from one that found nothing", () => {
    const el = root();
    renderPhone(el, state({ query: "zzz", searching: true }));
    expect(el.textContent).toContain("Searching");

    renderPhone(el, state({ query: "zzz", searching: false, results: [] }));
    expect(el.textContent).toContain("No matches");
  });

  it("escapes track metadata so a title cannot inject markup", () => {
    const el = root();
    renderPhone(
      el,
      state({
        music: music({
          now_playing: entry({ title: "<img src=x onerror=alert(1)>", artist: "x" }),
        }),
      }),
    );
    expect(el.querySelector("img[onerror]")).toBeNull();
    expect(el.textContent).toContain("<img src=x onerror=alert(1)>");
  });

  it("shows a placeholder rather than a blank line for an untitled track", () => {
    const el = root();
    renderPhone(el, state({ music: music({ now_playing: entry({ title: "" }) }) }));
    expect(el.textContent).toContain("Unknown track");
  });
});

describe("the search box across redraws", () => {
  // Typing one character fires a debounced search, which draws twice — once
  // for the spinner and once for the results — and the 3s poll draws again on
  // top. Rebuilding the input on any of those takes the caret and, on a phone,
  // closes the keyboard mid-word.
  it("keeps the very same input element alive", () => {
    const root = document.createElement("div");
    renderPhone(root, state());
    const first = root.querySelector<HTMLInputElement>(".search__input")!;
    expect(first).not.toBeNull();

    renderPhone(root, state());
    const second = root.querySelector<HTMLInputElement>(".search__input")!;
    expect(second).toBe(first);
  });

  it("keeps focus and what has been typed so far", () => {
    const root = document.createElement("div");
    document.body.append(root);
    renderPhone(root, state());

    const input = root.querySelector<HTMLInputElement>(".search__input")!;
    input.focus();
    input.value = "drinkin";
    input.setSelectionRange(4, 4);

    // A poll lands mid-word.
    renderPhone(root, { ...state(), searching: true });

    expect(document.activeElement).toBe(input);
    expect(input.value).toBe("drinkin");
    expect(input.selectionStart).toBe(4);
    root.remove();
  });

  // The one time the box *should* be cleared is when a track has been added:
  // the controller empties `query`, and the box has to follow.
  it("clears when the state clears the query", () => {
    const root = document.createElement("div");
    renderPhone(root, { ...state(), query: "drinkin" });
    const input = root.querySelector<HTMLInputElement>(".search__input")!;
    expect(input.value).toBe("drinkin");

    renderPhone(root, { ...state(), query: "" });
    expect(input.value).toBe("");
  });

  it("still redraws everything else", () => {
    const root = document.createElement("div");
    renderPhone(root, state());
    expect(root.textContent).toContain("Nothing playing");

    renderPhone(root, {
      ...state(),
      music: music({
        now_playing: entry({ title: "Drinkin' Problem", artist: "Midland" }),
      }),
    });
    expect(root.textContent).toContain("Drinkin' Problem");
    expect(root.textContent).not.toContain("Nothing playing");
  });
});
