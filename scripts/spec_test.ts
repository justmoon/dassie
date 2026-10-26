// The spec's links and the citations of it hold: every link inside spec/ and
// every `spec/….md#anchor` the repository cites names a file and heading that
// exist, nothing cites the chapters the spec no longer has (the original's
// among them, with their gap IDs such as `NET-24`), and every crate, app,
// package, test suite and contract has a row in spec/README.md's index.

import { test } from "node:test";
import { readdir, readFile } from "node:fs/promises";
import { dirname, join, relative } from "node:path";
import { assert, assertEquals } from "@std/assert";

const root = new URL("../", import.meta.url).pathname;

const SKIP = new Set([
  ".git",
  "node_modules",
  "target",
  "dist",
  "dist-dev",
  ".direnv",
]);
const TEXT = /\.(md|rs|ts|tsx|proto|nix|sh|ya?ml|toml|json|css|html)$/;

/** Every text file under `dir`, as paths relative to the repository. */
async function files(dir: string): Promise<string[]> {
  const found: string[] = [];
  for (const entry of await readdir(join(root, dir), { withFileTypes: true })) {
    if (SKIP.has(entry.name)) continue;
    const path = dir === "" ? entry.name : `${dir}/${entry.name}`;
    if (entry.isDirectory()) found.push(...await files(path));
    else if (TEXT.test(entry.name)) found.push(path);
  }
  return found;
}

/** The lines of a markdown file outside its code blocks. */
function prose(text: string): string[] {
  let fenced = false;
  return text.split("\n").map((line) => {
    if (line.startsWith("```")) {
      fenced = !fenced;
      return "";
    }
    return fenced ? "" : line;
  });
}

/** A heading's anchor, as GitHub makes it. */
export function slug(heading: string): string {
  return heading
    .trim()
    .toLowerCase()
    .replace(/<[^>]*>/g, "")
    .replace(/[^\p{L}\p{N}\p{M} _-]/gu, "")
    .replace(/ /g, "-");
}

/** The anchors of a markdown file's headings. */
function anchors(text: string): Set<string> {
  const seen = new Map<string, number>();
  const found = new Set<string>();
  for (const line of prose(text)) {
    const heading = /^#{1,6} (.*)$/.exec(line);
    if (!heading) continue;
    const base = slug(heading[1]);
    const count = seen.get(base) ?? 0;
    seen.set(base, count + 1);
    found.add(count === 0 ? base : `${base}-${count}`);
  }
  return found;
}

const all = await files("");
const spec = all.filter((path) =>
  path.startsWith("spec/") && path.endsWith(".md")
);
const texts = new Map<string, string>();
for (const path of all) {
  texts.set(path, await readFile(join(root, path), "utf8"));
}
const headings = new Map(spec.map((path) => [path, anchors(texts.get(path)!)]));

/** Why `target#anchor` does not resolve, or undefined if it does. */
function broken(
  target: string,
  anchor: string | undefined,
): string | undefined {
  if (!texts.has(target) && !all.includes(target)) {
    return "no such file";
  }
  if (anchor === undefined || !target.endsWith(".md")) return undefined;
  const known = headings.get(target) ?? anchors(texts.get(target)!);
  return known.has(anchor) ? undefined : `no heading #${anchor}`;
}

test("spec: every link inside the spec resolves", () => {
  const problems: string[] = [];
  for (const path of spec) {
    prose(texts.get(path)!).forEach((line, index) => {
      for (const [, href] of line.matchAll(/\]\(([^)\s]+)\)/g)) {
        // Skip URLs, and code such as `f[x](...)` outside backticks.
        if (/^[a-z]+:/.test(href) || !/[a-z]/i.test(href)) continue;
        const [file, anchor] = href.split("#");
        const target = file === ""
          ? path
          : relative(root, join(root, dirname(path), file));
        if (target.startsWith("..")) continue;
        const why = broken(target, anchor);
        if (why) problems.push(`${path}:${index + 1}: ${href}: ${why}`);
      }
    });
  }
  assertEquals(problems, []);
});

test("spec: every citation of the spec in the repository resolves", () => {
  const problems: string[] = [];
  for (const [path, text] of texts) {
    if (path.startsWith("spec/")) continue;
    text.split("\n").forEach((line, index) => {
      for (
        const [, file, anchor] of line.matchAll(
          /\bspec\/([\w./-]*\w\.md)(?:#([\p{L}\p{N}_-]+))?/gu,
        )
      ) {
        const why = broken(`spec/${file}`, anchor);
        if (why) problems.push(`${path}:${index + 1}: spec/${file}: ${why}`);
      }
    });
  }
  assertEquals(problems, []);
});

test("spec: nothing cites the chapters the spec no longer has", () => {
  const old = [
    /\b[Ss]pec [0-9]+ ?§/,
    /\b0[789]-[a-z-]+\.md/,
    /\bspec\/0[1-9]-/,
    /\bspec\/original\//,
    /\b(?:(?:RT|NET|ILP|ACC|UI)-[0-9]+|OER-q[0-9]+)\b/,
  ];
  const problems: string[] = [];
  for (const [path, text] of texts) {
    if (path === "scripts/spec_test.ts") continue;
    text.split("\n").forEach((line, index) => {
      if (old.some((pattern) => pattern.test(line))) {
        problems.push(`${path}:${index + 1}: ${line.trim()}`);
      }
    });
    // A citation that a comment wraps across lines.
    const joined = text.replace(/\s*\n\s*(?:\/\/[/!]?|#|\*|--)?\s*/g, " ");
    for (const [match] of joined.matchAll(/\b[Ss]pec [0-9]+ §/g)) {
      if (!problems.some((problem) => problem.startsWith(`${path}:`))) {
        problems.push(`${path}: ${match}`);
      }
    }
  }
  assertEquals(problems, []);
});

test("spec: every crate, app, package, test suite and contract is indexed", async () => {
  const index = texts.get("spec/README.md")!;
  for (const dir of ["crates", "apps", "packages", "tests", "proto/dassie"]) {
    for (
      const entry of await readdir(join(root, dir), { withFileTypes: true })
    ) {
      if (!entry.isDirectory()) continue;
      const path = `${dir}/${entry.name}`;
      const paths = dir === "proto/dassie"
        ? (await readdir(join(root, path))).map((version) =>
          `${path}/${version}`
        )
        : [path];
      for (const each of paths) {
        assert(
          index.includes(`\`${each}\``),
          `spec/README.md does not index ${each}`,
        );
      }
    }
  }
});

test("spec: the index of decisions lists each one once, by name", () => {
  // spec/decisions/named-decisions.md: in alphabetical order, so that two
  // pull requests adding a decision each add their row in a different place.
  const decisions = spec
    .filter((path) =>
      path.startsWith("spec/decisions/") && path !== "spec/decisions/README.md"
    )
    .map((path) => path.slice("spec/decisions/".length, -".md".length));
  const index = texts.get("spec/decisions/README.md")!;
  const table = index.slice(index.indexOf("| Decision | Title | Areas |"));
  const rows = [
    ...table.slice(0, table.indexOf("\n\n")).matchAll(
      /^\| \[([\w-]+)\]\(([\w-]+)\.md\) \|/gm,
    ),
  ];
  for (const [, name, file] of rows) assertEquals(name, file);
  const listed = rows.map(([, name]) => name);
  assertEquals(listed, [...decisions].sort());
  for (const name of decisions) {
    assert(/^[a-z0-9]+(?:-[a-z0-9]+)*$/.test(name), `${name}: not a name`);
  }
});

test("spec: nothing cites a decision by a number", () => {
  // spec/decisions/named-decisions.md. The former numbers' table only
  // translates them, and `D1.localhost` is a hostname.
  const number = /\bD[0-9]{1,3}\b(?!\.[Ll]ocal[Hh]ost\b)/;
  const problems: string[] = [];
  for (const [path, text] of texts) {
    if (path === "scripts/spec_test.ts") continue;
    text.split("\n").forEach((line, index) => {
      if (path === "spec/decisions/README.md" && /^\| D[0-9]+ \|/.test(line)) {
        return;
      }
      if (number.test(line)) {
        problems.push(`${path}:${index + 1}: ${line.trim()}`);
      }
    });
  }
  assertEquals(problems, []);
});

test("spec: slugs follow GitHub's rules", () => {
  assertEquals(slug("4.7 Peer protocol v1"), "47-peer-protocol-v1");
  assertEquals(
    slug("v0.2. Each node books its own Fulfills"),
    "v02-each-node-books-its-own-fulfills",
  );
  assertEquals(slug("A link's thresholds"), "a-links-thresholds");
  assertEquals(slug("`settlementRequest` [11]"), "settlementrequest-11");
});
