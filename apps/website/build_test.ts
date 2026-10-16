// What dassie.land's build must keep to:
//
// - the CSP in vercel.json: no inline script or style, no script, style or
//   image from elsewhere (links to elsewhere are fine);
// - every page has the shared header and footer;
// - vercel-build.sh installs the Node and pnpm the flake has and the
//   wasm-bindgen Cargo.lock has, so that Vercel builds what CI builds;
// - vercel-ignore.sh skips a deployment only when nothing the site is
//   built from changed, and watches every crate and package it is built
//   from.
//
// The checks of the pages run only once `node --run build:website` has run.

import { test } from "node:test";
import { assert, assertEquals } from "@std/assert";
import { execFile, spawn } from "node:child_process";
import { readdir, readFile } from "node:fs/promises";
import process from "node:process";
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
  "every page has the header and footer, and only one current page",
  { skip: !built },
  async () => {
    for (const { path } of PAGES) {
      const html = await readFile(`${dist}${path}index.html`, "utf8");
      assert(html.includes('<header class="site-header">'), path);
      assert(html.includes('<footer class="site-footer">'), path);
      assert(!html.includes("<!-- header -->"), path);
      assertEquals(html.match(/aria-current="page"/g)?.length, 1, path);
    }
  },
);

test("vercel-build.sh installs the flake's Node and pnpm and Cargo.lock's wasm-bindgen", async () => {
  const script = await readFile(
    new URL("vercel-build.sh", import.meta.url),
    "utf8",
  );
  const pin = (name: string) =>
    script.match(new RegExp(`^${name}=(\\S+)$`, "m"))?.[1];
  // CI runs in the flake's shell, so its Node and pnpm are the flake's.
  assertEquals(pin("NODE_VERSION"), process.versions.node);
  assertEquals(
    pin("PNPM_VERSION"),
    (await output("pnpm", ["--version"])).trim(),
  );
  const lock = await readFile(
    new URL("../../Cargo.lock", import.meta.url),
    "utf8",
  );
  const bindgen = lock.match(/name = "wasm-bindgen"\nversion = "([^"]+)"/)?.[1];
  assertEquals(pin("WASM_BINDGEN_VERSION"), bindgen);
  for (const name of ["NODE", "PNPM", "RUSTUP", "WASM_BINDGEN"]) {
    assert(/^[0-9a-f]{64}$/.test(pin(`${name}_SHA256`) ?? ""), name);
  }
});

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

/** The paths vercel-ignore.sh watches. */
async function watched(): Promise<string[]> {
  const script = await readFile(
    new URL("vercel-ignore.sh", import.meta.url),
    "utf8",
  );
  const list = script.slice(script.indexOf("HEAD -- \\"));
  return [...list.matchAll(/^ {2}(\S+)(?: \\)?$/gm)].map((match) => match[1]);
}

/** The workspace crates the WASM modules are built from. */
async function wasmCrates(): Promise<Set<string>> {
  const buildWasm = await readFile(`${root}scripts/build-wasm.ts`, "utf8");
  const roots = JSON.parse(
    buildWasm.match(/const crates = (\[[^\]]*\]);/)![1],
  ) as string[];
  const metadata = JSON.parse(
    await output("cargo", [
      "metadata",
      "--format-version=1",
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
    const sources = await output("git", [
      "grep",
      "-h",
      "-o",
      "-E",
      "@dassie/[a-z-]+",
      "--",
      dir,
    ]);
    for (const name of new Set(sources.split("\n").filter(Boolean))) {
      const imported = byName.get(name);
      if (imported) queue.push(imported);
    }
  }
  return packages;
}

test("vercel-ignore.sh watches every crate and package the site is built from", async () => {
  const paths = await watched();
  for (const dir of [...await wasmCrates(), ...await sitePackages()]) {
    assert(paths.includes(dir), `vercel-ignore.sh does not watch ${dir}`);
  }
});

test("vercel-ignore.sh skips only when it knows nothing changed", async () => {
  const site = new URL(".", import.meta.url).pathname;
  const run = async (previous?: string) => {
    const env = { ...process.env };
    delete env.VERCEL_GIT_PREVIOUS_SHA;
    if (previous !== undefined) env.VERCEL_GIT_PREVIOUS_SHA = previous;
    const child = spawn("sh", ["vercel-ignore.sh"], {
      cwd: site,
      env,
      stdio: "ignore",
    });
    return await new Promise<number | null>((resolve, reject) => {
      child.once("error", reject);
      child.once("close", resolve);
    });
  };
  // Skips: nothing can have changed since the commit itself.
  assertEquals(
    await run((await output("git", ["rev-parse", "HEAD"])).trim()),
    0,
  );
  // Builds: a first deployment, and a commit this clone does not have.
  assertEquals(await run(), 1);
  assertEquals(await run(""), 1);
  assertEquals(await run("0".repeat(40)), 1);
});
