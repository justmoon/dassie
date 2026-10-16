/**
 * What a built page of the wallet or the dev dashboard must keep to: their
 * CSP allows no inline script or style and nothing from other origins
 * (spec 7 §5), so the built templates hold no `style` attribute and
 * `index.html` no inline script or style.
 *
 * @module
 */

import { assert } from "@std/assert";
import { readdir, readFile, stat } from "node:fs/promises";

/** Whether `path` exists. */
export async function exists(path: string): Promise<boolean> {
  try {
    await stat(path);
    return true;
  } catch {
    return false;
  }
}

/** Fails on anything in the build at `dist` that the page CSP would refuse. */
export async function assertCspClean(dist: string): Promise<void> {
  const index = await readFile(`${dist}/index.html`, "utf8");
  assert(!/<script(?![^>]*\bsrc=)[^>]*>/.test(index), "an inline script");
  assert(!/<style/.test(index), "an inline style");
  assert(!/\bstyle=/.test(index), "a style attribute");
  assert(!/(src|href)="https?:/.test(index), "something from elsewhere");
  for (const [name, code] of await scripts(dist)) {
    // Solid compiles static markup into template strings: a style there
    // would be parsed from markup, which the CSP refuses.
    const templates = code.match(/`<[^`]*`/g) ?? [];
    for (const template of templates) {
      assert(!/\sstyle=/.test(template), `${name}: ${template}`);
    }
  }
}

/** The built scripts in `dist/assets`, by name. */
export async function scripts(dist: string): Promise<[string, string][]> {
  const found: [string, string][] = [];
  for (const name of await readdir(`${dist}/assets`)) {
    if (!name.endsWith(".js")) continue;
    found.push([name, await readFile(`${dist}/assets/${name}`, "utf8")]);
  }
  return found;
}
