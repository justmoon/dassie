// Kopje's drawings on the wallet's pages are the brand page's own files
// (AGENTS.md, "Follow the visual identity"). The wallet cannot reach the
// website's files in its build, so it keeps copies, which must not drift
// from them.

import { test } from "node:test";
import { assertEquals } from "@std/assert";
import { readFile } from "node:fs/promises";

const here = new URL("./", import.meta.url);
const brand = new URL("../../../website/brand/", import.meta.url);

/** Where the brand page keeps each drawing the wallet copies. */
const SOURCES: Record<string, string> = {
  "balance-grass.svg": "img/balance-grass.svg",
};

test("the wallet's drawings are the brand page's files", async () => {
  for (const [name, source] of Object.entries(SOURCES)) {
    const copy = await readFile(new URL(name, here));
    const original = await readFile(new URL(source, brand));
    assertEquals(copy.equals(original), true, `${name} differs from ${source}`);
  }
});
