// What the wallet's build must keep to:
//
// - the page's CSP (see `@dassie/ui/testing/csp`);
// - the production build has none of the development build's auto-login
//   (spec/areas/wallet.md#dev-auto-login), which reads a login proof from
//   the URL.
//
// The checks run only once `node --run build:wallet` has run.

import { test } from "node:test";
import { assert } from "@std/assert";
import { assertCspClean, exists, scripts } from "@dassie/ui/testing/csp";

const dist = new URL("dist", import.meta.url).pathname;
/** Where the build puts its content-hashed files (vite.config.ts). */
const HASHED = "static";
const built = await exists(`${dist}/index.html`);

test(
  "the built page has no inline script or style, and nothing from elsewhere",
  { skip: !built },
  () => assertCspClean(dist, HASHED),
);

test(
  "the production build cannot log in from the URL",
  { skip: !built },
  async () => {
    for (const [name, code] of await scripts(dist, HASHED)) {
      assert(!code.includes("dassie-dev-login"), name);
    }
  },
);
