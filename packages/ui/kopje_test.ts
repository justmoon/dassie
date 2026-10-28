// The mark and Kopje's drawings are the brand page's own files
// (AGENTS.md, "Take the mark from apps/website/public/brand/; never redraw
// it"), and Kopje's fonts this package's. The wallet cannot reach the
// website's files in its build, nor the desktop app's page this package's,
// so each keeps copies, which must not drift from them.

import { test } from "node:test";
import { assertEquals } from "@std/assert";
import { readdir, readFile } from "node:fs/promises";

const here = new URL("./kopje/", import.meta.url);
const desktop = new URL("../../crates/dassie-desktop/ui/", import.meta.url);
const website = new URL("../../apps/website/", import.meta.url);

/** Where the brand page keeps each file this package copies. */
const SOURCES: Record<string, string> = {
  "dassie-mark.svg": "public/brand/dassie-mark.svg",
  "dassie-mark-small.svg": "public/brand/dassie-mark-small.svg",
};

test("the mark's copies are the brand page's files", async () => {
  const copies = (await readdir(here)).filter((name) => name.endsWith(".svg"));
  assertEquals(copies.sort(), Object.keys(SOURCES).sort());
  for (const [name, source] of Object.entries(SOURCES)) {
    const copy = await readFile(new URL(name, here));
    const original = await readFile(new URL(source, website));
    assertEquals(copy.equals(original), true, `${name} differs from ${source}`);
  }
});

test("the desktop app's starting page has Kopje's own files", async () => {
  const copies: Record<string, URL> = {
    "dassie-mark.svg": new URL("public/brand/dassie-mark.svg", website),
    "fonts/fraunces.woff2": new URL("fonts/fraunces.woff2", here),
    "fonts/figtree.woff2": new URL("fonts/figtree.woff2", here),
    "fonts/OFL-Fraunces.txt": new URL("fonts/OFL-Fraunces.txt", here),
    "fonts/OFL-Figtree.txt": new URL("fonts/OFL-Figtree.txt", here),
  };
  for (const [name, original] of Object.entries(copies)) {
    const copy = await readFile(new URL(name, desktop));
    const source = await readFile(original);
    assertEquals(copy.equals(source), true, `${name} differs from ${original}`);
  }
});
