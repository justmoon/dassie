import { test } from "node:test";
import { execFile } from "node:child_process";
import { readdir, readFile, stat } from "node:fs/promises";
import { dirname } from "node:path";
import { promisify } from "node:util";
import { assert, assertEquals } from "@std/assert";
import {
  type ArchitectureMap,
  BOX_HEIGHT,
  edgePath,
  highlight,
  MAP,
} from "./map.ts";

const root = new URL("../../../../", import.meta.url).pathname;
const KINDS = ["uses", "talks", "wasm", "runs"];

const unique = (values: string[]) => new Set(values).size === values.length;

/** Whether two boxes share any area. */
const overlap = (
  a: { x: number; y: number; w: number; h?: number },
  b: { x: number; y: number; w: number; h?: number },
) =>
  a.x < b.x + b.w && b.x < a.x + a.w &&
  a.y < b.y + (b.h ?? BOX_HEIGHT) && b.y < a.y + (a.h ?? BOX_HEIGHT);

test("map.json: every subsystem has a colour in site.css", async () => {
  const css = await readFile(new URL("../site.css", import.meta.url), "utf8");
  assert(unique(MAP.subsystems.map((sub) => sub.id)));
  for (const { id } of MAP.subsystems) {
    assert(css.includes(`.sub-${id} {`), `site.css has no .sub-${id}`);
  }
});

test("map.json: every crate, app, package and contract has a box", async () => {
  const paths = new Set(Object.values(MAP.components).map((box) => box.path));
  for (const dir of ["crates", "apps", "packages", "tests", "proto/dassie"]) {
    for (
      const entry of await readdir(`${root}${dir}`, { withFileTypes: true })
    ) {
      if (!entry.isDirectory()) continue;
      const path = `${dir}/${entry.name}`;
      if (dir === "proto/dassie") {
        for (const version of await readdir(`${root}${path}`)) {
          const contract = `${path}/${version}`;
          assert(paths.has(contract), `${contract} has no box`);
        }
      } else {
        assert(paths.has(path), `${path} has no box`);
      }
    }
  }
  for (const [id, box] of Object.entries(MAP.components)) {
    await stat(`${root}${box.path}`).catch(() => {
      throw new Error(`${id}: ${box.path} does not exist`);
    });
  }
});

test("map.json: the boxes sit in a layer, apart, inside the map", () => {
  const boxes = Object.entries(MAP.components);
  for (const [id, box] of boxes) {
    assert(
      MAP.subsystems.some((sub) => sub.id === box.sub),
      `${id}: subsystem ${box.sub}`,
    );
    assert(
      MAP.bands.some((band) =>
        box.x >= band.x && box.x + box.w <= band.x + band.w &&
        box.y >= band.y && box.y + (box.h ?? BOX_HEIGHT) <= band.y + band.h
      ),
      `${id} is in no layer`,
    );
    for (const [other, next] of boxes) {
      if (other !== id) assert(!overlap(box, next), `${id} overlaps ${other}`);
    }
  }
  for (const band of MAP.bands) {
    assert(band.x >= 0 && band.x + band.w <= MAP.viewBox.w, band.t);
    assert(band.y >= 0 && band.y + band.h <= MAP.viewBox.h, band.t);
  }
});

test("map.json: every link and journey step names boxes", () => {
  for (const edge of MAP.edges) {
    const name = `${edge.f} → ${edge.t}`;
    assert(edge.f in MAP.components, name);
    assert(edge.t in MAP.components, name);
    assert(KINDS.includes(edge.k), `${name}: ${edge.k}`);
    assertEquals(typeof edge.h, "boolean", name);
  }
  assert(unique(MAP.flows.map((flow) => flow.id)));
  for (const flow of MAP.flows) {
    for (const step of flow.steps) {
      for (const id of step.ids) {
        assert(id in MAP.components, `${flow.id}: ${id}`);
      }
    }
  }
});

/**
 * What each crate and package builds on, by path, as its manifest says: a
 * crate's normal dependencies (its dev- and build-dependencies serve only
 * its tests and build script), and a package's `workspace:` dependencies
 * (the test packages list theirs as devDependencies).
 */
async function manifestDependencies(): Promise<Map<string, string[]>> {
  const deps = new Map<string, string[]>();
  const relative = (dir: string) => dir.slice(root.length);
  const { stdout } = await promisify(execFile)(
    "cargo",
    ["metadata", "--format-version=1", "--no-deps", "--offline"],
    { cwd: root, maxBuffer: 16 << 20 },
  );
  for (const crate of JSON.parse(stdout).packages) {
    deps.set(
      relative(dirname(crate.manifest_path)),
      crate.dependencies
        .filter((dep: { kind: string | null; path?: string }) =>
          dep.kind === null && dep.path !== undefined
        )
        .map((dep: { path: string }) => relative(dep.path)),
    );
  }
  const packages = new Map<string, { path: string; uses: string[] }>();
  for (const dir of ["apps", "packages", "tests"]) {
    for (
      const entry of await readdir(`${root}${dir}`, { withFileTypes: true })
    ) {
      if (!entry.isDirectory()) continue;
      const path = `${dir}/${entry.name}`;
      const json = JSON.parse(
        await readFile(`${root}${path}/package.json`, "utf8"),
      );
      const all = { ...json.dependencies, ...json.devDependencies };
      const uses = Object.keys(all).filter((dep) =>
        all[dep].startsWith("workspace:")
      );
      packages.set(json.name, { path, uses });
    }
  }
  for (const { path, uses } of packages.values()) {
    deps.set(path, uses.map((name) => packages.get(name)!.path));
  }
  return deps;
}

test("map.json: a box uses what its manifest depends on", async () => {
  const ids = new Map(
    Object.entries(MAP.components).map(([id, box]) => [box.path, id]),
  );
  for (const [path, deps] of await manifestDependencies()) {
    const id = ids.get(path)!;
    const uses = MAP.edges
      .filter((edge) => edge.f === id && edge.k === "uses")
      .map((edge) => edge.t);
    assertEquals(
      uses.sort(),
      deps.map((dep) => ids.get(dep)!).sort(),
      `${id} uses`,
    );
  }
});

test("map.json: the newest release is the one Cargo.toml names", async () => {
  const manifest = await readFile(
    `${root}crates/dassie-cli/Cargo.toml`,
    "utf8",
  );
  const version = manifest.match(/^version = "([^"]+)"/m)?.[1];
  assertEquals(MAP.releases[0]?.version, version);
  const dates = MAP.releases.map((release) => release.date);
  assertEquals(dates, [...dates].sort().reverse());
  for (const release of MAP.releases) {
    assert(/^\d{4}-\d{2}-\d{2}$/.test(release.date), release.date);
    assert(release.changes.length > 0, release.version);
  }
});

/** A map of three boxes in a row, a → b → c, and a hidden a → c. */
const small = {
  components: { a: {}, b: {}, c: {} },
  edges: [
    { f: "a", t: "b", k: "uses", h: false },
    { f: "b", t: "c", k: "uses", h: false },
    { f: "a", t: "c", k: "uses", h: true },
  ],
  flows: [{
    id: "pay",
    steps: [{ ids: ["a"] }, { ids: ["b", "a"] }, { ids: ["c"] }],
  }],
} as unknown as ArchitectureMap;

test("highlight: a box lights every link it has, hidden ones too", () => {
  const lit = highlight(small, { type: "component", id: "a" });
  assertEquals([...lit.boxes!].sort(), ["a", "b", "c"]);
  assertEquals([...lit.edges].sort(), [0, 2]);
  assertEquals(highlight(small, { type: "none" }).boxes, null);
});

test("highlight: a journey numbers each box at its first step", () => {
  const lit = highlight(small, { type: "flow", id: "pay" });
  assertEquals([...lit.badges], [["a", 1], ["b", 2], ["c", 3]]);
  assertEquals([...lit.edges].sort(), [0, 1]);
});

test("edgePath: across for side by side, a curve between layers", () => {
  const a = { x: 0, y: 0, w: 100 }, b = { x: 200, y: 0, w: 100 };
  assertEquals(edgePath(a, b), "M100,30 L200,30");
  assertEquals(edgePath(b, a), "M200,30 L100,30");
  const below = { x: 0, y: 200, w: 100 };
  assertEquals(edgePath(a, below), "M50,60 C50,130 50,130 50,200");
});
