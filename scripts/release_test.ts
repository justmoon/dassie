// scripts/release.ts against a stand-in release, signed with the test key
// of testdata/release. minisign comes from the flake's dev shell, and
// dassie-release from `node --run build:e2e`; without them the tests skip,
// except in CI, where they fail.

import { test } from "node:test";
import { assertEquals, assertRejects } from "@std/assert";
import { spawn } from "node:child_process";
import { statSync } from "node:fs";
import { mkdir, mkdtemp, readdir, readFile, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import process from "node:process";
import { files, release } from "./release.ts";

const testdata = new URL("../testdata/release/", import.meta.url).pathname;
const secretKey = `${testdata}test.key`;
const publicKey = `${testdata}test.pub`;

/** Whether `command` runs, and succeeds. */
function succeeds(command: string, args: string[]): Promise<boolean> {
  return new Promise((resolve) => {
    const child = spawn(command, args, { stdio: "ignore" });
    child.once("error", () => resolve(false));
    child.once("close", (code) => resolve(code === 0));
  });
}

const hasMinisign = await succeeds("minisign", ["-v"]);
const root = new URL("../", import.meta.url).pathname;
const verifier = [
  `${root}target/e2e/dassie-release`,
  `${root}target/debug/dassie-release`,
]
  .find((path) => {
    try {
      return statSync(path).isFile();
    } catch {
      return false;
    }
  });
if ((!hasMinisign || !verifier) && process.env.CI) {
  throw new Error(
    "minisign or dassie-release is missing: run `node --run build:e2e` in `nix develop`",
  );
}

/** A built tree as `nix build .#release` makes it, with stand-in files. */
async function builtTree(version: string, meta = "latest"): Promise<string> {
  const tree = await mkdtemp(join(tmpdir(), "dassie-release-"));
  await mkdir(`${tree}/meta`);
  await mkdir(`${tree}/${version}`);
  await writeFile(`${tree}/meta/${meta}`, version);
  const lines = [];
  for (const name of files(version)) {
    const bytes = new TextEncoder().encode(`stand-in for ${name}`);
    await writeFile(`${tree}/${version}/${name}`, bytes);
    const digest = new Uint8Array(await crypto.subtle.digest("SHA-256", bytes));
    lines.push(
      `${
        Array.from(digest, (b) => b.toString(16).padStart(2, "0")).join("")
      }  ${name}\n`,
    );
  }
  await writeFile(`${tree}/${version}/SHA256SUMS`, lines.join(""));
  await writeFile(`${tree}/install.sh`, "stand-in for install.sh");
  return tree;
}

const options = { skip: !hasMinisign || !verifier };
const tools = { secretKey, publicKey, verifier: verifier ?? "" };

test(
  "signs a release, and the signature names its version",
  options,
  async () => {
    const tree = await builtTree("0.1.0");
    const out = `${tree}-out`;
    const built = await release({ tree, out, ...tools });
    assertEquals(built, { version: "0.1.0" });
    assertEquals(await readFile(`${out}/meta/latest`, "utf8"), "0.1.0");
    assertEquals(
      await readFile(`${out}/install.sh`, "utf8"),
      "stand-in for install.sh",
    );
    const signature = await readFile(
      `${out}/0.1.0/SHA256SUMS.minisig`,
      "utf8",
    );
    assertEquals(signature.split("\n")[2], "trusted comment: dassie 0.1.0");
    assertEquals(
      (await readdir(`${out}/0.1.0`)).sort(),
      [...files("0.1.0"), "SHA256SUMS", "SHA256SUMS.minisig"].sort(),
    );
  },
);

test("refuses a bundle that does not match its sum", options, async () => {
  const tree = await builtTree("0.1.0");
  await writeFile(
    `${tree}/0.1.0/dassie-0.1.0-linux-arm64.tar.gz`,
    "tampered",
  );
  await assertRejects(
    () => release({ tree, out: `${tree}-out`, ...tools }),
    Error,
    "dassie-0.1.0-linux-arm64.tar.gz does not match SHA256SUMS",
  );
});

test(
  "refuses to publish what the public key does not verify",
  options,
  async () => {
    const tree = await builtTree("0.1.0");
    const other = await mkdtemp(join(tmpdir(), "dassie-key-"));
    const generated = await succeeds("minisign", [
      "-G",
      "-W",
      "-p",
      `${other}/other.pub`,
      "-s",
      `${other}/other.key`,
    ]);
    assertEquals(generated, true);
    await assertRejects(
      () =>
        release({
          tree,
          out: `${tree}-out`,
          ...tools,
          publicKey: `${other}/other.pub`,
        }),
      Error,
      "dassie-release verify failed",
    );
  },
);

test("refuses a tree of another version than the tag's", options, async () => {
  const tree = await builtTree("0.1.0");
  await assertRejects(
    () =>
      release({
        tree,
        out: `${tree}-out`,
        ...tools,
        expect: "0.1.1",
      }),
    Error,
    "the tree holds 0.1.0, not 0.1.1",
  );
});

test("refuses a meta/latest that names no version", options, async () => {
  const tree = await builtTree("canary-2026-10-15-041530-15bd7aa");
  await assertRejects(
    () => release({ tree, out: `${tree}-out`, ...tools }),
    Error,
    "not a version",
  );
});

test("refuses a tree of another channel", options, async () => {
  const tree = await builtTree("0.1.0", "canary");
  await assertRejects(
    () => release({ tree, out: `${tree}-out`, ...tools }),
    Error,
    "not latest alone",
  );
});
