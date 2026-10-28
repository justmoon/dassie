// What the docs at dassie.land/docs/ must keep to, checked on their
// sources without a build: src/docs-layout.ts renders each page as the
// build does.
//
// - Every page in the page list (src/docs.ts) has its file, and every file
//   under docs/ is in the list.
// - Each page has the title, description and heading the list names it
//   by, keeps to the CSP, and every link on it to a page of the site, or
//   to an anchor, leads to one that exists. A link to the repository on
//   GitHub names a file that exists; links into spec/ are spec_test.ts's.
// - The reference pages have an entry, an element whose id is its name,
//   for everything the code defines: each subcommand of the dassie command
//   (/docs/reference/cli/, `dassie-<command>` and `dassie-hub-<command>`),
//   each DASSIE_* variable the released binaries read
//   (/docs/reference/environment/), each settlement scheme
//   (/docs/reference/schemes/) and each option of the NixOS module
//   (/docs/reference/nixos-options/, `services.dassie.<option>`). Each list
//   is read from the code, not kept here.
//
// Each page is a test of its own, named by its path, so that a writer
// checks their own:
//
//   node --test --test-name-pattern=/docs/node/status/ apps/website/docs_test.ts

import { test } from "node:test";
import { existsSync, readdirSync, readFileSync, statSync } from "node:fs";
import { join } from "node:path";
import { assert, assertEquals } from "@std/assert";
import { docsPaths, inDocs, SECTIONS, SIDEBAR } from "./src/docs.ts";
import { renderDocs, sourceFile, text } from "./src/docs-layout.ts";
import { footer, header, sitePaths } from "./src/layout.ts";

const site = new URL("./", import.meta.url).pathname;
const root = new URL("../../", import.meta.url).pathname;

/** The page at `path`, as the build makes it, but for its assets. */
const rendered = new Map<string, string>();
function render(path: string): string {
  let html = rendered.get(path);
  if (html === undefined) {
    const source = readFileSync(sourceFile(path), "utf8");
    html = (inDocs(path) ? renderDocs(path, source) : source)
      .replace("<!-- header -->", header(path))
      .replace("<!-- footer -->", footer());
    rendered.set(path, html);
  }
  return html;
}

/** The ids on the page at `path`, or why there are none. */
function ids(path: string): Set<string> | string {
  try {
    return new Set(
      [...render(path).matchAll(/\sid="([^"]+)"/g)].map((match) => match[1]),
    );
  } catch (error) {
    return String(error);
  }
}

/** HTML's entities that the pages use, as text. */
function decode(html: string): string {
  return html.replaceAll("&quot;", '"').replaceAll("&#39;", "'")
    .replaceAll("&#x27;", "'").replaceAll("&lt;", "<").replaceAll("&gt;", ">")
    .replaceAll("&nbsp;", " ").replaceAll("&amp;", "&");
}

const pages = new Set(sitePaths());
const GITHUB = "https://github.com/justmoon/dassie/";

/** Why the link `href` on the page at `path` leads nowhere, if it does. */
function broken(path: string, href: string): string | undefined {
  const [target, anchor] = decode(href).split("#", 2);
  if (target === "") {
    const own = ids(path);
    if (typeof own === "string") return own;
    return own.has(anchor) ? undefined : "no such anchor on this page";
  }
  if (target.startsWith("/")) {
    const page = target.split("?")[0];
    if (!pages.has(page)) {
      return existsSync(join(site, "public", page))
        ? undefined
        : page.endsWith("/")
        ? "no such page"
        : "no such page or file (a page's path ends in a slash)";
    }
    if (anchor === undefined) return undefined;
    const there = ids(page);
    if (typeof there === "string") return `${page} does not render: ${there}`;
    return there.has(anchor) ? undefined : `no #${anchor} on ${page}`;
  }
  if (/^https?:\/\/(www\.)?dassie\.land\//.test(target)) {
    return "a link to dassie.land itself: write its path";
  }
  const file = target.match(
    /^https:\/\/github\.com\/justmoon\/dassie\/(?:blob|tree)\/main\/(.+)$/,
  )?.[1];
  if (file !== undefined) {
    // spec_test.ts checks links into the spec, with the spec at hand;
    // the TypeScript checks run without it, and without the top-level
    // Markdown files (nix/ts.nix).
    if (file.startsWith("spec/") || !file.includes("/")) return undefined;
    return existsSync(join(root, file))
      ? undefined
      : "no such file in the repository";
  } else if (
    target.startsWith(GITHUB) &&
    !/\/(issues|pulls|discussions|releases)\b/.test(target)
  ) {
    return "a link into the repository names the main branch: /blob/main/<path>";
  }
  return undefined;
}

/** The pages' files under docs/, by their path. */
function docsFiles(dir = join(site, "docs"), path = "/docs/"): string[] {
  const found: string[] = [];
  for (const entry of readdirSync(dir, { withFileTypes: true })) {
    if (entry.isDirectory()) {
      found.push(
        ...docsFiles(join(dir, entry.name), `${path}${entry.name}/`),
      );
    } else if (entry.name.endsWith(".html")) {
      found.push(entry.name === "index.html" ? path : `${path}${entry.name}`);
    }
  }
  return found;
}

test("the page list and the files under docs/ match", () => {
  const listed = docsPaths();
  assertEquals(
    new Set(listed).size,
    listed.length,
    "a page is in the list twice",
  );
  for (const page of SIDEBAR) {
    if (!inDocs(page.path)) {
      assert(pages.has(page.path), `${page.path}: not a page of the site`);
    }
  }
  const files = docsFiles();
  const missing = listed.filter((path) => !files.includes(path));
  const unlisted = files.filter((path) => !listed.includes(path));
  assertEquals(missing, [], "listed in src/docs.ts, with no file");
  assertEquals(
    unlisted,
    [],
    "a file under docs/ that src/docs.ts doesn't list",
  );
});

for (const path of docsPaths()) {
  test(path, () => {
    assert(existsSync(sourceFile(path)), `${path}: no file`);
    const html = render(path);
    const problems: string[] = [];

    const page = SIDEBAR.find((page) => page.path === path);
    const section = SECTIONS.find((section) => section.path === path);
    const title = page?.title ?? section?.name;
    const pageTitle = decode(html.match(/<title>([^<]*)<\/title>/)?.[1] ?? "");
    const h1s = [...html.matchAll(/<h1\b[^>]*>([\s\S]*?)<\/h1>/g)];
    if (h1s.length !== 1) problems.push(`${h1s.length} <h1>s, not one`);
    if (title !== undefined) {
      if (pageTitle !== `${title} · Dassie`) {
        problems.push(`<title> is "${pageTitle}", not "${title} · Dassie"`);
      }
      const h1 = decode(text(h1s[0]?.[1] ?? ""));
      if (h1s.length === 1 && h1 !== title) {
        problems.push(`<h1> is "${h1}", not "${title}"`);
      }
    }
    const description = html.match(
      /<meta\s+name="description"\s+content="([^"]*)"/,
    )?.[1].trim();
    if (!description) problems.push('no <meta name="description">');
    else if (description.length > 200) {
      problems.push(
        `a description of ${description.length} characters, more than 200`,
      );
    }

    if (/<script(?![^>]*\bsrc=)[^>]*>/.test(html)) {
      problems.push("an inline script");
    }
    if (/<style/.test(html)) problems.push("an inline style");
    if (/\sstyle=/.test(html)) problems.push("a style attribute");
    if (/\s(src|srcset|poster)="(https?:)?\/\//.test(html)) {
      problems.push("something loaded from elsewhere");
    }

    const seen = new Set<string>();
    for (const [, id] of html.matchAll(/\sid="([^"]+)"/g)) {
      if (seen.has(id)) problems.push(`the id "${id}" twice`);
      seen.add(id);
    }
    for (const [, href] of html.matchAll(/<a\b[^>]*?\shref="([^"]*)"/g)) {
      const why = broken(path, href);
      if (why) problems.push(`${href}: ${why}`);
    }
    assertEquals(problems, [], path);
  });
}

// ---------------------------------------------------------------------
// The reference pages against the code.

/** The repository's file at `path`. */
function source(path: string): string {
  return readFileSync(join(root, path), "utf8");
}

/** The entries of `expected` that the reference page at `path` has no id
 * for. */
function uncovered(path: string, expected: Iterable<string>): string[] {
  const found = ids(path);
  if (typeof found === "string") throw new Error(found);
  return [...expected].filter((id) => !found.has(id)).sort();
}

/**
 * The words a `match` on a string matches in its own arms, the arms
 * indented as its first: the arms of `match ... {` from the line after
 * `start` in `code`, to the arm that starts with `end`. Each arm starts with
 * `"word" =>` or `word @ ("a" | "b") =>`.
 */
function arms(code: string, start: string, end: string): string[] {
  const from = code.indexOf(start);
  assert(from >= 0, `no ${start}`);
  const lines = code.slice(from).split("\n").slice(1);
  const indent = lines[0].match(/^ */)![0];
  const words: string[] = [];
  for (const line of lines) {
    if (!line.startsWith(indent) || line[indent.length] === " ") continue;
    const arm = line.slice(indent.length);
    if (arm.startsWith(end)) return words;
    const head = arm.match(/^(?:\w+ @ \()?("[\w-]+"(?: \| "[\w-]+")*)\)? =>/);
    if (head) {
      words.push(...head[1].split(" | ").map((word) => word.slice(1, -1)));
    }
  }
  throw new Error(`no ${end} after ${start}`);
}

test("/docs/reference/cli/ has an entry for every subcommand of the dassie command", () => {
  const main = source("crates/dassie-cli/src/main.rs");
  const commands = arms(
    main,
    "Value(command) => match command.string()?.as_str() {",
    "other =>",
  );
  assert(commands.includes("hub"), commands.join());
  const hub = [
    ...arms(
      source("crates/dassie-cli/src/hub.rs"),
      "Some(Value(word)) => match word.string()?.as_str() {",
      "other =>",
    ),
    ...arms(
      source("crates/dassie-cli/src/founding.rs"),
      "let command = match word {",
      "other =>",
    ),
  ];
  assert(hub.includes("deposit") && hub.includes("found"), hub.join());
  assertEquals(
    uncovered(
      "/docs/reference/cli/",
      new Set([
        ...commands.map((command) => `dassie-${command}`),
        ...hub.map((command) => `dassie-hub-${command}`),
      ]),
    ),
    [],
  );
});

/** The workspace crates `crate` (a directory under crates/) is built
 * from: its own, and those of its normal dependencies, not its dev- or
 * build-dependencies. */
function closure(crates: string[]): Set<string> {
  const found = new Set<string>();
  const pending = [...crates];
  while (pending.length > 0) {
    const crate = pending.pop()!;
    if (found.has(crate)) continue;
    found.add(crate);
    let table = "";
    for (const line of source(`crates/${crate}/Cargo.toml`).split("\n")) {
      const header = line.match(/^\[(.+)\]\s*$/);
      if (header) table = header[1];
      const normal = table === "dependencies" ||
        /^target\..+\.dependencies$/.test(table);
      const path = line.match(/path\s*=\s*"\.\.\/([\w-]+)"/)?.[1];
      if (normal && path) pending.push(path);
    }
  }
  return found;
}

/** Every `.rs` file under `dir`, but test modules of their own. */
function rustFiles(dir: string): string[] {
  const found: string[] = [];
  for (const entry of readdirSync(join(root, dir), { withFileTypes: true })) {
    const path = `${dir}/${entry.name}`;
    if (entry.isDirectory()) {
      if (entry.name !== "tests") found.push(...rustFiles(path));
    } else if (
      entry.name.endsWith(".rs") && !/(^|_)tests?\.rs$/.test(entry.name)
    ) {
      found.push(path);
    }
  }
  return found;
}

/**
 * Rust source without its comments, and without the items compiled only
 * for tests and simulations: those under `#[cfg(test)]` or a feature for
 * them (`test-key`, `testing`, `sim`).
 */
function released(code: string): string {
  let out = "";
  let i = 0;
  const testOnly =
    /#\[cfg\((?!not\()[^\]]*(\btest\b|"test-key"|"testing"|"sim")[^\]]*\)\]/y;
  while (i < code.length) {
    testOnly.lastIndex = i;
    if (testOnly.test(code)) {
      i = skipItem(code, testOnly.lastIndex);
      continue;
    }
    const next = token(code, i);
    out += next.text;
    i = next.end;
  }
  return out;
}

/** The token at `i`: a comment (as a space), a string or character
 * literal, or one character. */
function token(code: string, i: number): { text: string; end: number } {
  if (code.startsWith("//", i)) {
    const end = code.indexOf("\n", i);
    return { text: " ", end: end < 0 ? code.length : end };
  }
  if (code.startsWith("/*", i)) {
    let depth = 0;
    let j = i;
    do {
      if (code.startsWith("/*", j)) {
        depth++;
        j += 2;
      } else if (code.startsWith("*/", j)) {
        depth--;
        j += 2;
      } else j++;
    } while (depth > 0 && j < code.length);
    return { text: " ", end: j };
  }
  const raw = code.slice(i).match(/^b?r(#*)"/);
  if (raw) {
    const close = `"${raw[1]}`;
    const end = code.indexOf(close, i + raw[0].length) + close.length;
    return { text: code.slice(i, end), end };
  }
  if (code[i] === '"') {
    let j = i + 1;
    while (j < code.length && code[j] !== '"') j += code[j] === "\\" ? 2 : 1;
    return { text: code.slice(i, j + 1), end: j + 1 };
  }
  const char = code.slice(i).match(/^'(?:\\.[^']*|[^'\\])'/);
  if (char) return { text: char[0], end: i + char[0].length };
  return { text: code[i], end: i + 1 };
}

/** Where the item, statement, field or arm that starts at `i` ends: at a
 * `;` or `,` outside brackets, after the `}` that closes its body, or
 * before the bracket that closes what holds it. */
function skipItem(code: string, i: number): number {
  let depth = 0;
  let j = i;
  while (j < code.length) {
    const next = token(code, j);
    if ("([{".includes(next.text)) depth++;
    else if (")]}".includes(next.text)) {
      depth--;
      if (depth < 0) return j;
      if (depth === 0 && next.text === "}") return next.end;
    } else if ((next.text === ";" || next.text === ",") && depth === 0) {
      return next.end;
    }
    j = next.end;
  }
  return j;
}

test("/docs/reference/environment/ has an entry for every DASSIE_* variable the released binaries read", () => {
  // The binaries a release ships: `dassie` and the desktop app.
  const crates = closure(["dassie-cli", "dassie-desktop"]);
  const variables = new Set<string>();
  for (const crate of crates) {
    for (const file of rustFiles(`crates/${crate}/src`)) {
      const code = released(source(file));
      for (
        const [, before, name] of code.matchAll(
          /((?:option_)?env!\(\s*)?"(DASSIE_[A-Z0-9_]+)"/g,
        )
      ) {
        // Read at build time, not by the binary.
        if (!before) variables.add(name);
      }
    }
  }
  assert(variables.has("DASSIE_LOG"), [...variables].join());
  assertEquals(uncovered("/docs/reference/environment/", variables), []);
});

test("/docs/reference/schemes/ has an entry for every settlement scheme the node can run", () => {
  const schemes = source("crates/dassie-node/src/engine/core/schemes.rs");
  const list = schemes.match(
    /pub const SCHEMES: &\[SchemeInfo\] = &\[([\s\S]*?)\n\];/,
  )?.[1];
  assert(list, "schemes.rs has no SCHEMES");
  const ids = [...list.matchAll(/\bid: ([^,]+),/g)].map(([, id]) => {
    const literal = id.match(/^"([^"]+)"$/)?.[1];
    if (literal) return literal;
    // A constant of a workspace crate, as `dassie_hub::member::SCHEME_ID`:
    // in the module's file, or in the files of its directory.
    const [crate, ...rest] = id.split("::");
    const name = rest.pop()!;
    const dir = `crates/${crate.replaceAll("_", "-")}/src/${rest.join("/")}`;
    const files = [
      `${dir}.rs`,
      ...(existsSync(join(root, dir)) && statSync(join(root, dir)).isDirectory()
        ? rustFiles(dir)
        : []),
    ].filter((file) => existsSync(join(root, file)));
    const values = new Set(
      files.flatMap((file) =>
        [
          ...released(source(file)).matchAll(
            new RegExp(`pub const ${name}: &str = "([^"]+)";`, "g"),
          ),
        ].map(([, value]) => value)
      ),
    );
    assertEquals(values.size, 1, `${id}: ${[...values].join()}`);
    return [...values][0];
  });
  assert(ids.length > 0);
  assertEquals(uncovered("/docs/reference/schemes/", ids), []);
});

test("/docs/reference/nixos-options/ has an entry for every option of the NixOS module", () => {
  const module = source("nix/flake/module.nix");
  const start = module.indexOf("options.services.dassie = {");
  assert(start >= 0, "module.nix has no options.services.dassie");
  // The block, to its matching brace.
  let depth = 0;
  let end = module.indexOf("{", start);
  for (; end < module.length; end++) {
    if (module[end] === "{") depth++;
    else if (module[end] === "}" && --depth === 0) break;
  }
  const block = module.slice(module.indexOf("{", start) + 1, end);
  const options: string[] = [];
  depth = 0;
  for (const line of block.split("\n")) {
    const name = line.match(
      /^\s*([A-Za-z][\w-]*)\s*=\s*lib\.mk(?:Enable|Package)?Option\b/,
    )?.[1];
    if (depth === 0 && name) options.push(`services.dassie.${name}`);
    for (const char of line.replace(/"(?:\\.|[^"\\])*"/g, "")) {
      if (char === "{") depth++;
      else if (char === "}") depth--;
    }
  }
  assert(options.includes("services.dassie.enable"), options.join());
  assertEquals(uncovered("/docs/reference/nixos-options/", options), []);
});
