// What the wallet's build and sources must keep to:
//
// - the page's CSP allows no inline script or style and nothing from other
//   origins (spec 7 §5), so the built templates hold no `style` attribute
//   and `index.html` no inline script or style;
// - `src/shared` imports nothing else of the wallet, so that the dev
//   dashboard (M5b) can take it over as `packages/ui` unchanged.
//
// The build checks run only once `deno task build:wallet` has run.

import { assert, assertEquals } from "@std/assert";

const root = new URL(".", import.meta.url).pathname;
const dist = `${root}dist`;

async function exists(path: string): Promise<boolean> {
  try {
    await Deno.stat(path);
    return true;
  } catch {
    return false;
  }
}

const built = await exists(`${dist}/index.html`);

Deno.test({
  name:
    "the built page has no inline script or style, and nothing from elsewhere",
  ignore: !built,
  async fn() {
    const index = await Deno.readTextFile(`${dist}/index.html`);
    assert(!/<script(?![^>]*\bsrc=)[^>]*>/.test(index), "an inline script");
    assert(!/<style/.test(index), "an inline style");
    assert(!/\bstyle=/.test(index), "a style attribute");
    assert(!/(src|href)="https?:/.test(index), "something from elsewhere");
    for await (const entry of Deno.readDir(`${dist}/assets`)) {
      if (!entry.name.endsWith(".js")) continue;
      const code = await Deno.readTextFile(`${dist}/assets/${entry.name}`);
      // Solid compiles static markup into template strings: a style there
      // would be parsed from markup, which the CSP refuses.
      const templates = code.match(/`<[^`]*`/g) ?? [];
      for (const template of templates) {
        assert(!/\sstyle=/.test(template), `${entry.name}: ${template}`);
      }
    }
  },
});

Deno.test("src/shared imports only itself, Solid and the RPC packages", async () => {
  const allowed = [
    "solid-js",
    "@solidjs/web",
    "@dassie/rpc",
    "@dassie/rpc/proto",
  ];
  const outside: string[] = [];
  const walk = async (dir: string) => {
    for await (const entry of Deno.readDir(dir)) {
      const path = `${dir}/${entry.name}`;
      if (entry.isDirectory) {
        await walk(path);
        continue;
      }
      if (!/\.tsx?$/.test(entry.name) || entry.name.includes("_test.")) {
        continue;
      }
      const code = await Deno.readTextFile(path);
      for (const [, specifier] of code.matchAll(/from "([^"]+)"/g)) {
        if (allowed.includes(specifier)) continue;
        if (!specifier.startsWith(".")) {
          outside.push(`${path}: ${specifier}`);
          continue;
        }
        const target = new URL(specifier, `file://${path}`).pathname;
        if (!target.startsWith(`${root}src/shared/`)) {
          outside.push(`${path}: ${specifier}`);
        }
      }
    }
  };
  await walk(`${root}src/shared`);
  assertEquals(outside, []);
});
