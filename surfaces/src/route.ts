/**
 * Which surface a path belongs to (D9).
 *
 * Its own module rather than a function in `main.ts`, because `main.ts` starts
 * a surface as a side effect of being imported — importing it to test the
 * routing would boot the TV inside the test runner.
 */

/** The three things this bundle can be. */
export type Surface = "tv" | "join" | "phone";

/** Pick a surface from `location.pathname`. */
export function surfaceFor(pathname: string): Surface {
  if (pathname.startsWith("/join")) return "join";
  if (pathname.startsWith("/phone")) return "phone";
  // Everything else is the TV. `seshd` serves `index.html` for unknown paths,
  // and a mistyped URL on the one screen with no keyboard should land on the
  // home grid rather than an error page.
  return "tv";
}
