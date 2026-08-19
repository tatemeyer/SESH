/** The join screen: the one thing between scanning a QR and having the queue. */

/** Everything the join screen renders from. */
export interface JoinState {
  /** The code from the QR's query string, or null when arrived at by hand. */
  code: string | null;
  /** What is typed in the name field. */
  name: string;
  /** True while the join request is in flight. */
  joining: boolean;
  /** Why the last attempt failed. */
  notice?: string | null;
}

function escapeHtml(value: string): string {
  return value
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(/"/g, "&quot;");
}

/** Render the join screen into `root`, replacing its contents. */
export function renderJoin(root: HTMLElement, state: JoinState): void {
  if (state.code === null) {
    // Landing here without a code means a bookmark, a typed URL, or a code
    // that has already rotated away. Point at the TV rather than showing a
    // form that cannot succeed.
    root.innerHTML = `<main class="join">
      <h1 class="wordmark">SESH</h1>
      <p class="join__lead">Scan the code on the TV to join.</p>
    </main>`;
    return;
  }

  const notice = state.notice
    ? `<p class="banner banner--error">${escapeHtml(state.notice)}</p>`
    : "";

  root.innerHTML = `<main class="join">
    <h1 class="wordmark">SESH</h1>
    <p class="join__lead">What should the room call you?</p>
    ${notice}
    <form class="join__form">
      <input class="join__name" name="name" type="text" inputmode="text"
        autocomplete="nickname" maxlength="40" placeholder="Your name"
        value="${escapeHtml(state.name)}" />
      <button class="join__go" type="submit"${state.joining ? " disabled" : ""}>
        ${state.joining ? "Joining…" : "Join"}
      </button>
    </form>
  </main>`;
}
