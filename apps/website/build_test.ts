// What dassie.land's build must keep to:
//
// - the CSP in vercel.json: no inline script or style, no script, style or
//   image from elsewhere (links to elsewhere are fine);
// - every page has the shared header and footer;
// - vercel-build.sh installs the Deno the flake has and the wasm-bindgen
//   Cargo.lock has, so that Vercel builds what CI builds;
// - vercel-ignore.sh skips a deployment only when nothing the site is
//   built from changed, and watches every crate and package it is built
//   from.
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

const root = new URL("../../", import.meta.url).pathname;

/** What `command` prints, run at the repository's root. */
async function output(command: string, args: string[]): Promise<string> {
  const result = await new Deno.Command(command, { args, cwd: root }).output();
  assert(result.success, `${command} ${args.join(" ")} failed`);
  return new TextDecoder().decode(result.stdout);
}

/** The paths vercel-ignore.sh watches. */
async function watched(): Promise<string[]> {
  const script = await Deno.readTextFile(
    new URL("vercel-ignore.sh", import.meta.url),
  );
  const list = script.slice(script.indexOf("HEAD -- \\"));
  return [...list.matchAll(/^ {2}(\S+)(?: \\)?$/gm)].map((match) => match[1]);
}

/** The workspace crates the WASM modules are built from. */
async function wasmCrates(): Promise<Set<string>> {
  const buildWasm = await Deno.readTextFile(`${root}scripts/build-wasm.ts`);
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

/** The workspace packages the site imports, and those they import. */
async function sitePackages(): Promise<Set<string>> {
  const manifest = JSON.parse(await Deno.readTextFile(`${root}deno.json`)) as {
    workspace: string[];
  };
  const byName = new Map<string, string>();
  for (const member of manifest.workspace) {
    const dir = member.replace(/^\.\//, "");
    const { name } = JSON.parse(
      await Deno.readTextFile(`${root}${dir}/deno.json`),
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

Deno.test("vercel-ignore.sh watches every crate and package the site is built from", async () => {
  const paths = await watched();
  for (const dir of [...await wasmCrates(), ...await sitePackages()]) {
    assert(paths.includes(dir), `vercel-ignore.sh does not watch ${dir}`);
  }
});

Deno.test("vercel-ignore.sh skips only when it knows nothing changed", async () => {
  const site = new URL(".", import.meta.url).pathname;
  const run = async (previous?: string) => {
    const env = { ...Deno.env.toObject() };
    delete env.VERCEL_GIT_PREVIOUS_SHA;
    if (previous !== undefined) env.VERCEL_GIT_PREVIOUS_SHA = previous;
    const result = await new Deno.Command("sh", {
      args: ["vercel-ignore.sh"],
      cwd: site,
      env,
      clearEnv: true,
    }).output();
    return result.code;
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
