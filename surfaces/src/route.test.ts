import { describe, expect, it } from "vitest";
import { surfaceFor } from "./route";

describe("surfaceFor", () => {
  it("sends the root to the TV", () => {
    expect(surfaceFor("/")).toBe("tv");
  });

  it("sends a scanned QR to the join screen", () => {
    expect(surfaceFor("/join")).toBe("join");
    // The code arrives as a query string, which is not part of the pathname,
    // but a trailing slash is.
    expect(surfaceFor("/join/")).toBe("join");
  });

  it("sends a joined phone to the queue", () => {
    expect(surfaceFor("/phone")).toBe("phone");
  });

  // The TV has no keyboard and no back button. Whatever a stray path is, the
  // home grid is a better landing place than an error.
  it("falls back to the TV for anything else", () => {
    expect(surfaceFor("/nonsense")).toBe("tv");
    expect(surfaceFor("")).toBe("tv");
  });
});
