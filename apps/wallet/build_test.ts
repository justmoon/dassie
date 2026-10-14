// What the wallet's build must keep to:
//
// - the page's CSP (see `@dassie/ui/testing/csp`);
// - the production build has none of the development build's auto-login
//   (spec 7 §6), which reads a login proof from the URL.
//
// The checks run only once `deno task build:wallet` has run.

import { assert } from "@std/assert";
import { assertCspClean, exists, scripts } from "@dassie/ui/testing/csp";

const dist = new URL("dist", import.meta.url).pathname;
const built = await exists(`${dist}/index.html`);

Deno.test({
  name:
    "the built page has no inline script or style, and nothing from elsewhere",
  ignore: !built,
  fn: () => assertCspClean(dist),
});

Deno.test({
  name: "the production build cannot log in from the URL",
  ignore: !built,
  async fn() {
    for (const [name, code] of await scripts(dist)) {
      assert(!code.includes("dassie-dev-login"), name);
    }
  },
});
