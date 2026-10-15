// What dassie.land's build must keep to:
//
// - the CSP in vercel.json: no inline script or style, no script, style or
//   image from elsewhere (links to elsewhere are fine);
// - every page has the shared header and footer;
// - vercel-build.sh installs the Deno the flake has and the wasm-bindgen
//   Cargo.lock has, so that Vercel builds what CI builds.
//
// The checks of the pages run only once `deno task build:website` has run.

import { assert, assertEquals } from "@std/assert";
import { exists, scripts } from "@dassie/ui/testing/csp";
import { PAGES } from "./src/layout.ts";

const dist = new URL("dist", import.meta.url).pathname;
const built = await exists(`${dist}/index.html`);

Deno.test({
  name:
    "the pages have no inline script or style, and load nothing from elsewhere",
  ignore: !built,
  async fn() {
    for (const { path } of PAGES) {
      const html = await Deno.readTextFile(`${dist}${path}index.html`);
      assert(
        !/<script(?![^>]*\bsrc=)[^>]*>/.test(html),
        `${path}: inline script`,
      );
      assert(!/<style/.test(html), `${path}: inline style`);
      assert(!/\bstyle=/.test(html), `${path}: style attribute`);
      assert(!/\bsrc="(https?:)?\/\//.test(html), `${path}: source elsewhere`);
      assert(
        !/<link[^>]*href="(https?:)?\/\//.test(html),
        `${path}: link elsewhere`,
      );
    }
    for (const [name, code] of await scripts(dist)) {
      for (const template of code.match(/`<[^`]*`/g) ?? []) {
        assert(!/\sstyle=/.test(template), `${name}: ${template}`);
      }
    }
  },
});

Deno.test({
  name: "every page has the header and footer, and only one current page",
  ignore: !built,
  async fn() {
    for (const { path } of PAGES) {
      const html = await Deno.readTextFile(`${dist}${path}index.html`);
      assert(html.includes('<header class="site-header">'), path);
      assert(html.includes('<footer class="site-footer">'), path);
      assert(!html.includes("<!-- header -->"), path);
      assertEquals(html.match(/aria-current="page"/g)?.length, 1, path);
    }
  },
});

Deno.test("vercel-build.sh installs the flake's Deno and Cargo.lock's wasm-bindgen", async () => {
  const script = await Deno.readTextFile(
    new URL("vercel-build.sh", import.meta.url),
  );
  const pin = (name: string) =>
    script.match(new RegExp(`^${name}=(\\S+)$`, "m"))?.[1];
  // CI runs in the flake's shell, so its Deno is the flake's.
  assertEquals(pin("DENO_VERSION"), Deno.version.deno);
  const lock = await Deno.readTextFile(
    new URL("../../Cargo.lock", import.meta.url),
  );
  const bindgen = lock.match(/name = "wasm-bindgen"\nversion = "([^"]+)"/)?.[1];
  assertEquals(pin("WASM_BINDGEN_VERSION"), bindgen);
  for (const name of ["DENO", "RUSTUP", "WASM_BINDGEN"]) {
    assert(/^[0-9a-f]{64}$/.test(pin(`${name}_SHA256`) ?? ""), name);
  }
});
