// scripts/release.ts against a stand-in release, signed with the test key
// of testdata/release. minisign comes from the flake's dev shell, and
// dassie-release from `deno task build:e2e`; without them the tests skip,
// except in CI, where they fail.

import { assertEquals, assertRejects } from "@std/assert";
import { bundles, release } from "./release.ts";

const testdata = new URL("../testdata/release/", import.meta.url).pathname;
const secretKey = `${testdata}test.key`;
const publicKey = `${testdata}test.pub`;

const hasMinisign = await new Deno.Command("minisign", {
  args: ["-v"],
  stdout: "null",
})
  .output()
  .then((result) => result.success, () => false);
const root = new URL("../", import.meta.url).pathname;
const verifier = [
  `${root}target/e2e/dassie-release`,
  `${root}target/debug/dassie-release`,
]
  .find((path) => {
    try {
      return Deno.statSync(path).isFile;
    } catch {
      return false;
    }
  });
if ((!hasMinisign || !verifier) && Deno.env.get("CI")) {
  throw new Error(
    "minisign or dassie-release is missing: run `deno task build:e2e` in `nix develop`",
  );
}

/** A built tree as `nix build .#release` makes it, with stand-in bundles. */
async function builtTree(version: string, channel = "latest"): Promise<string> {
  const tree = await Deno.makeTempDir({ prefix: "dassie-release-" });
  await Deno.mkdir(`${tree}/meta`);
  await Deno.mkdir(`${tree}/${version}`);
  await Deno.writeTextFile(`${tree}/meta/${channel}`, version);
  const lines = [];
  for (const name of bundles(version)) {
    const bytes = new TextEncoder().encode(`stand-in for ${name}`);
    await Deno.writeFile(`${tree}/${version}/${name}`, bytes);
    const digest = new Uint8Array(await crypto.subtle.digest("SHA-256", bytes));
    lines.push(
      `${
        Array.from(digest, (b) => b.toString(16).padStart(2, "0")).join("")
      }  ${name}\n`,
    );
  }
  await Deno.writeTextFile(`${tree}/${version}/SHA256SUMS`, lines.join(""));
  return tree;
}

const options = { ignore: !hasMinisign || !verifier };
const tools = { secretKey, publicKey, verifier: verifier ?? "" };

Deno.test(
  "signs a release, and the signature names its version",
  options,
  async () => {
    const tree = await builtTree("0.1.0");
    const out = `${tree}-out`;
    const built = await release({ tree, out, ...tools });
    assertEquals(built, { channel: "latest", version: "0.1.0" });
    assertEquals(await Deno.readTextFile(`${out}/meta/latest`), "0.1.0");
    const signature = await Deno.readTextFile(
      `${out}/0.1.0/SHA256SUMS.minisig`,
    );
    assertEquals(signature.split("\n")[2], "trusted comment: dassie 0.1.0");
    const files = [];
    for await (const entry of Deno.readDir(`${out}/0.1.0`)) {
      files.push(
        entry.name,
      );
    }
    assertEquals(
      files.sort(),
      [...bundles("0.1.0"), "SHA256SUMS", "SHA256SUMS.minisig"].sort(),
    );
  },
);

Deno.test("signs a canary", options, async () => {
  const tree = await builtTree("canary-2026-10-15-041530-15bd7aa", "canary");
  const built = await release({
    tree,
    out: `${tree}-out`,
    ...tools,
  });
  assertEquals(built, {
    channel: "canary",
    version: "canary-2026-10-15-041530-15bd7aa",
  });
});

Deno.test("refuses a bundle that does not match its sum", options, async () => {
  const tree = await builtTree("0.1.0");
  await Deno.writeTextFile(
    `${tree}/0.1.0/dassie-0.1.0-linux-arm64.tar.xz`,
    "tampered",
  );
  await assertRejects(
    () => release({ tree, out: `${tree}-out`, ...tools }),
    Error,
    "dassie-0.1.0-linux-arm64.tar.xz does not match SHA256SUMS",
  );
});

Deno.test(
  "refuses to publish what the public key does not verify",
  options,
  async () => {
    const tree = await builtTree("0.1.0");
    const other = await Deno.makeTempDir({ prefix: "dassie-key-" });
    const generated = await new Deno.Command("minisign", {
      args: [
        "-G",
        "-W",
        "-p",
        `${other}/other.pub`,
        "-s",
        `${other}/other.key`,
      ],
      stdout: "null",
    }).output();
    assertEquals(generated.success, true);
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

Deno.test(
  "refuses a tree of another version than the tag's",
  options,
  async () => {
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
  },
);

Deno.test("refuses a channel file that names no version", options, async () => {
  const tree = await builtTree("canary-2026-10-15-15bd7aa", "canary");
  await assertRejects(
    () => release({ tree, out: `${tree}-out`, ...tools }),
    Error,
    "not a canary version",
  );
});
