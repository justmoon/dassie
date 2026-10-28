// What dassie.land's build must keep to:
//
// - the CSP in headers.json: no inline script or style, no script, style or
//   image from elsewhere (links to elsewhere are fine);
// - every page has the shared header and footer, and every page under
//   /docs/ the docs layout;
// - the docs are indexed for search, without Pagefind's own interfaces;
// - the website workflow publishes on a change to anything the site is
//   built from: its `paths` name every crate and package it is built from.
//
// The checks of the pages run only once `node --run build:website` has run.
// docs_test.ts checks the docs' pages themselves, without a build.

import { test } from "node:test";
import { assert, assertEquals } from "@std/assert";
import { execFile } from "node:child_process";
import { readdir, readFile } from "node:fs/promises";
import { exists, scripts } from "@dassie/ui/testing/csp";
import { inDocs } from "./src/docs.ts";
import { EARLY_DAYS, NAV, sitePaths } from "./src/layout.ts";

const root = new URL("../../", import.meta.url).pathname;
const dist = new URL("dist", import.meta.url).pathname;
const built = await exists(`${dist}/index.html`);

/** Every HTML file under `dir`, by its path from it. */
async function htmlFiles(dir: string, prefix = ""): Promise<string[]> {
  const found: string[] = [];
  for (const entry of await readdir(dir, { withFileTypes: true })) {
    const path = `${prefix}/${entry.name}`;
    if (entry.isDirectory()) {
      found.push(...await htmlFiles(`${dir}/${entry.name}`, path));
    } else if (entry.name.endsWith(".html")) found.push(path);
  }
  return found;
}

test(
  "the pages have no inline script or style, and load nothing from elsewhere",
  { skip: !built },
  async () => {
    const files = await htmlFiles(dist);
    assertEquals(
      files.toSorted(),
      sitePaths().map((path) => `${path}index.html`).toSorted(),
      "dist/ holds a page the site doesn't list, or misses one",
    );
    for (const path of files) {
      const html = await readFile(`${dist}${path}`, "utf8");
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
);

test(
  "every page has the header and footer, and the header marks it current when it links it",
  { skip: !built },
  async () => {
    const linked = new Set([
      "/",
      ...NAV.map((page) => page.path),
      EARLY_DAYS.path,
    ]);
    for (const path of sitePaths()) {
      const html = await readFile(`${dist}${path}index.html`, "utf8");
      const header = html.match(
        /<header class="site-header">[\s\S]*?<\/header>/,
      )
        ?.[0];
      assert(header, `${path}: no header`);
      assert(html.includes('<footer class="site-footer">'), path);
      assert(!html.includes("<!-- header -->"), path);
      assert(!html.includes("<!-- footer -->"), path);
      const isLinked = linked.has(path);
      assertEquals(
        header.match(/aria-current="page"/g)?.length ?? 0,
        isLinked ? 1 : 0,
        path,
      );
      assertEquals(header.includes(`href="${path}"`), isLinked, path);
      // The header links the brand page nowhere: AGENTS.md and the docs'
      // Project section do.
      assert(!header.includes('href="/brand/"'), path);
    }
  },
);

test(
  "every page under /docs/ is in the docs layout, and the docs are indexed for search",
  { skip: !built },
  async () => {
    for (const path of sitePaths().filter(inDocs)) {
      const html = await readFile(`${dist}${path}index.html`, "utf8");
      assert(html.includes('<div class="docs-layout'), path);
      assert(html.includes('<nav class="docs-nav"'), path);
      assert(!/<main class="docs">/.test(html), path);
      assert(!/<!-- (releases|newest-release|sections) -->/.test(html), path);
    }
    const index = await readdir(`${dist}/pagefind`);
    assert(index.includes("pagefind.js"), index.join());
    assert(index.includes("pagefind-entry.json"), index.join());
    assert(
      !index.some((name) => /ui|highlight/.test(name)),
      `Pagefind's own interfaces: ${index.join()}`,
    );
  },
);

/** What `command` prints, run at the repository's root. */
function output(command: string, args: string[]): Promise<string> {
  return new Promise((resolve, reject) => {
    execFile(
      command,
      args,
      { cwd: root, maxBuffer: 64 * 1024 * 1024 },
      (error, stdout) => {
        if (error) {
          reject(
            new Error(`${command} ${args.join(" ")} failed`, {
              cause: error,
            }),
          );
        } else resolve(stdout);
      },
    );
  });
}

/** The paths whose change publishes the site, from website.yml. */
async function watched(): Promise<string[]> {
  const workflow = await readFile(
    `${root}.github/workflows/website.yml`,
    "utf8",
  );
  const list = workflow.match(/^ {4}paths:\n((?:(?: *#.*| {6}- .*)\n)+)/m)?.[1];
  assert(list, "website.yml has no paths");
  return [...list.matchAll(/^ {6}- (\S+)$/gm)].map((match) =>
    match[1].replace(/\/\*\*$/, "")
  );
}

/**
 * The workspace crates the WASM modules are built from: the bindings crate
 * and what it depends on, with every feature, since each module is built
 * with one of them.
 */
async function wasmCrates(): Promise<Set<string>> {
  const buildWasm = await readFile(`${root}scripts/build-wasm.ts`, "utf8");
  const bindings = buildWasm.match(/const crate = "([^"]+)";/)?.[1];
  assert(bindings, "build-wasm.ts names no crate");
  const roots = [bindings];
  const metadata = JSON.parse(
    await output("cargo", [
      "metadata",
      "--format-version=1",
      "--all-features",
      "--filter-platform=wasm32-unknown-unknown",
    ]),
  ) as {
    packages: { id: string; name: string; manifest_path: string }[];
    workspace_members: string[];
    resolve: {
      nodes: {
        id: string;
        deps: { pkg: string; dep_kinds: { kind: string | null }[] }[];
      }[];
    };
  };
  const members = new Set(metadata.workspace_members);
  const byId = new Map(metadata.packages.map((p) => [p.id, p]));
  const nodes = new Map(metadata.resolve.nodes.map((n) => [n.id, n]));
  const pending = metadata.packages
    .filter((p) => members.has(p.id) && roots.includes(p.name))
    .map((p) => p.id);
  assertEquals(pending.length, roots.length);
  const crates = new Set<string>();
  while (pending.length > 0) {
    const id = pending.pop()!;
    const dir = byId.get(id)!.manifest_path
      .slice(root.length, -"/Cargo.toml".length);
    if (crates.has(dir)) continue;
    crates.add(dir);
    for (const dep of nodes.get(id)!.deps) {
      const normal = dep.dep_kinds.some((kind) => kind.kind === null);
      if (normal && members.has(dep.pkg)) pending.push(dep.pkg);
    }
  }
  return crates;
}

/**
 * The directories of the workspace's packages: pnpm-workspace.yaml's
 * `packages`, each a directory or `dir/*`, with a package.json.
 */
async function workspaceMembers(): Promise<string[]> {
  const manifest = await readFile(`${root}pnpm-workspace.yaml`, "utf8");
  const list = manifest.match(/^packages:\n((?:[ \t]+- .*\n)+)/m)?.[1];
  assert(list, "pnpm-workspace.yaml lists no packages");
  const members: string[] = [];
  for (const [, glob] of list.matchAll(/- ['"]?([^'"\s]+)['"]?/g)) {
    const dirs = glob.endsWith("/*")
      ? (await readdir(`${root}${glob.slice(0, -2)}`, { withFileTypes: true }))
        .filter((entry) => entry.isDirectory())
        .map((entry) => `${glob.slice(0, -2)}/${entry.name}`)
      : [glob];
    for (const dir of dirs) {
      if (await exists(`${root}${dir}/package.json`)) members.push(dir);
    }
  }
  return members;
}

/**
 * What a checkout leaves out of a member (.gitignore): its installed
 * packages and its built output.
 */
const UNTRACKED = new Set(["node_modules", "dist", "dist-dev"]);

/**
 * The `@dassie/` packages the files under `dir` name. It reads the files
 * rather than asking git, since the Nix check runs without a checkout.
 */
async function named(dir: string): Promise<Set<string>> {
  const names = new Set<string>();
  for (const entry of await readdir(dir, { withFileTypes: true })) {
    const path = `${dir}/${entry.name}`;
    if (entry.isDirectory() && !UNTRACKED.has(entry.name)) {
      for (const name of await named(path)) names.add(name);
    } else if (entry.isFile()) {
      const text = await readFile(path, "utf8");
      for (const [name] of text.matchAll(/@dassie\/[a-z-]+/g)) names.add(name);
    }
  }
  return names;
}

/** The workspace packages the site imports, and those they import. */
async function sitePackages(): Promise<Set<string>> {
  const byName = new Map<string, string>();
  for (const dir of await workspaceMembers()) {
    const { name } = JSON.parse(
      await readFile(`${root}${dir}/package.json`, "utf8"),
    ) as { name?: string };
    if (name) byName.set(name, dir);
  }
  const packages = new Set<string>();
  const queue = ["apps/website"];
  while (queue.length > 0) {
    const dir = queue.pop()!;
    if (packages.has(dir)) continue;
    packages.add(dir);
    for (const name of await named(`${root}${dir}`)) {
      const imported = byName.get(name);
      if (imported) queue.push(imported);
    }
  }
  return packages;
}

test("the website workflow watches every crate and package the site is built from", async () => {
  const paths = await watched();
  for (const dir of [...await wasmCrates(), ...await sitePackages()]) {
    assert(paths.includes(dir), `website.yml does not watch ${dir}`);
  }
});
