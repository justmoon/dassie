// What dassie.land's build must keep to:
//
// - the CSP in headers.json: no inline script or style, no script, style or
//   image from elsewhere (links to elsewhere are fine);
// - every page has the shared header and footer;
// - the website workflow publishes on a change to anything the site is
//   built from: its `paths` name every crate and package it is built from.
//
// The checks of the pages run only once `node --run build:website` has run.

import { test } from "node:test";
import { assert, assertEquals } from "@std/assert";
import { execFile } from "node:child_process";
import { readdir, readFile } from "node:fs/promises";
import { exists, scripts } from "@dassie/ui/testing/csp";
import { PAGES } from "./src/layout.ts";

const root = new URL("../../", import.meta.url).pathname;
const dist = new URL("dist", import.meta.url).pathname;
const built = await exists(`${dist}/index.html`);

test(
  "the pages have no inline script or style, and load nothing from elsewhere",
  { skip: !built },
  async () => {
    for (const { path } of PAGES) {
      const html = await readFile(`${dist}${path}index.html`, "utf8");
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
  "every page has the header and footer, and the navigation marks it current when it lists it",
  { skip: !built },
  async () => {
    for (const { path, nav } of PAGES) {
      const html = await readFile(`${dist}${path}index.html`, "utf8");
      assert(html.includes('<header class="site-header">'), path);
      assert(html.includes('<footer class="site-footer">'), path);
      assert(!html.includes("<!-- header -->"), path);
      assertEquals(
        html.match(/aria-current="page"/g)?.length ?? 0,
        nav ? 1 : 0,
        path,
      );
      assertEquals(html.includes(`href="${path}"`), nav, path);
    }
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
