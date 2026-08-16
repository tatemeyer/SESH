import { describe, expect, it } from "vitest";
import { move } from "./nav";

describe("move", () => {
  // A 5-item grid, 3 columns:  0 1 2
  //                            3 4
  const COUNT = 5;
  const COLUMNS = 3;

  it("moves right within a row", () => {
    expect(move(0, COUNT, COLUMNS, "right")).toBe(1);
  });

  it("stops at the last item instead of wrapping", () => {
    expect(move(4, COUNT, COLUMNS, "right")).toBe(4);
  });

  it("stops at the first item instead of wrapping", () => {
    expect(move(0, COUNT, COLUMNS, "left")).toBe(0);
  });

  it("moves down a full row", () => {
    expect(move(0, COUNT, COLUMNS, "down")).toBe(3);
  });

  it("clamps to the last item when the row below is short", () => {
    expect(move(2, COUNT, COLUMNS, "down")).toBe(4);
  });

  it("moves up a full row", () => {
    expect(move(3, COUNT, COLUMNS, "up")).toBe(0);
  });

  it("stays put when there is no row above", () => {
    expect(move(1, COUNT, COLUMNS, "up")).toBe(1);
  });

  it("stays put when there is no row below", () => {
    expect(move(3, COUNT, COLUMNS, "down")).toBe(3);
  });

  it("returns 0 for an empty grid", () => {
    expect(move(0, 0, COLUMNS, "right")).toBe(0);
  });
});
