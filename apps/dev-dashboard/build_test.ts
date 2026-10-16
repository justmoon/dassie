// The built dashboard keeps to the same CSP as the wallet (see
// `@dassie/ui/testing/csp`). The check runs once `deno task
// build:dashboard` has run.

import { test } from "node:test";
import { assertCspClean, exists } from "@dassie/ui/testing/csp";

const dist = new URL("dist", import.meta.url).pathname;

test(
  "the built dashboard has no inline script or style, and nothing from elsewhere",
  { skip: !(await exists(`${dist}/index.html`)) },
  () => assertCspClean(dist),
);
