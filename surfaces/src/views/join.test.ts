import { describe, expect, it } from "vitest";
import { renderJoin } from "./join";

function root(): HTMLElement {
  const el = document.createElement("div");
  document.body.appendChild(el);
  return el;
}

describe("renderJoin", () => {
  it("asks for a name when it has a code", () => {
    const el = root();
    renderJoin(el, { code: "abc123", name: "", joining: false });

    expect(el.textContent).toContain("What should the room call you?");
    expect(el.querySelector('input[name="name"]')).not.toBeNull();
  });

  // Arriving without a code means a bookmark, a typed URL, or a code that has
  // rotated away. A form that cannot possibly succeed is worse than a
  // sentence pointing at the TV.
  it("points at the TV when there is no code", () => {
    const el = root();
    renderJoin(el, { code: null, name: "", joining: false });

    expect(el.textContent).toContain("Scan the code on the TV");
    expect(el.querySelector("form")).toBeNull();
  });

  it("disables the button while joining", () => {
    const el = root();
    renderJoin(el, { code: "abc123", name: "Sam", joining: true });

    const button = el.querySelector<HTMLButtonElement>(".join__go")!;
    expect(button.disabled).toBe(true);
    expect(button.textContent).toContain("Joining");
  });

  it("keeps what was typed across a re-render", () => {
    const el = root();
    renderJoin(el, { code: "abc123", name: "Marcus", joining: false });

    const input = el.querySelector<HTMLInputElement>('input[name="name"]')!;
    expect(input.value).toBe("Marcus");
  });

  it("shows why the last attempt failed", () => {
    const el = root();
    renderJoin(el, {
      code: "abc123",
      name: "Sam",
      joining: false,
      notice: "That code has expired. Scan the one on the TV again.",
    });
    expect(el.textContent).toContain("expired");
  });

  it("escapes a typed name so it cannot inject markup", () => {
    const el = root();
    renderJoin(el, { code: "abc", name: '"><img src=x onerror=alert(1)>', joining: false });

    expect(el.querySelector("img[onerror]")).toBeNull();
    const input = el.querySelector<HTMLInputElement>('input[name="name"]')!;
    expect(input.value).toBe('"><img src=x onerror=alert(1)>');
  });
});
