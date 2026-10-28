// What dassie.land's load balancer answers beyond its bucket, as JSON
// for scripts/website/provision.sh:
//
//   node apps/website/src/routes.ts
//
// - `pages`: every page's path but the home page's. The load balancer
//   redirects each without its slash to it, where the bucket would send
//   it to <page>/index.html;
// - `moved`: pages that moved, `from` their old path `to` the new one. The
//   load balancer redirects the old path, with its slash and without.

import { MOVED } from "./docs.ts";
import { sitePaths } from "./layout.ts";

console.log(JSON.stringify({
  pages: sitePaths().filter((path) => path !== "/"),
  moved: MOVED,
}));
