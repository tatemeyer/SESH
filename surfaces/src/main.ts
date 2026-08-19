/**
 * Bootstrap: pick a surface from the path and start it (D9).
 *
 * No router and no framework. `seshd` already serves `index.html` for unknown
 * paths precisely so the surface owns its routing, and one bundle with a
 * switch here costs a phone a couple of KB of TV code it never runs — cheaper
 * than a second entry point, a second HTML route, and a build change.
 */

import "./styles.css";
import { startJoin, startPhone } from "./phone";
import { surfaceFor } from "./route";
import { startTv } from "./tv";

const root = document.getElementById("app")!;

switch (surfaceFor(location.pathname)) {
  case "join":
    startJoin(root, location.search);
    break;
  case "phone":
    startPhone(root);
    break;
  default:
    startTv(root);
}
