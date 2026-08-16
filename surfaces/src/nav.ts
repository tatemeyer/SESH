/** Grid navigation. Pure logic, deliberately separate from the DOM. */

export type Dir = "up" | "down" | "left" | "right";

/**
 * The index selected after moving `dir` from `index` in a `columns`-wide
 * grid of `count` items. Movement clamps rather than wraps: on a TV,
 * wrapping loses people.
 */
export function move(index: number, count: number, columns: number, dir: Dir): number {
  if (count <= 0) return 0;

  const clamp = (i: number) => Math.max(0, Math.min(count - 1, i));

  switch (dir) {
    case "left":
      return index % columns === 0 ? index : clamp(index - 1);
    case "right":
      return (index + 1) % columns === 0 || index + 1 >= count ? index : clamp(index + 1);
    case "up":
      return index - columns < 0 ? index : index - columns;
    case "down":
      return index + columns >= count
        ? (Math.floor(index / columns) + 1) * columns >= count
          ? index
          : clamp(count - 1)
        : index + columns;
  }
}
