// Signs a release for the mirror (spec/decisions/release-integrity.md),
// as the release workflow does:
//
//   node scripts/release.ts <built tree> <out dir> \
//     --secret-key <file> --public-key resources/minisign.pub \
//     --verifier <dassie-release binary> [--version <expected version>]
//
// The built tree is `nix build .#release` or `.#canary`: meta/<channel>
// and <version>/ with the bundles and SHA256SUMS. The script copies it to
// <out dir>, checks every bundle against SHA256SUMS, and signs SHA256SUMS
// with minisign (SHA256SUMS.minisig, trusted comment "dassie <version>").
// Then `dassie-release verify` checks the signed tree with the public key
// the binaries trust, and with the code the updater trusts a release with:
// a release signed with any other key, or in a way the updater refuses,
// never reaches the mirror. The secret key's password, if it has one,
// comes from MINISIGN_PASSWORD.
//
// minisign comes from the flake's dev shell, and dassie-release from
// `nix build .#release-tool` or `node --run build:e2e`.

import { spawn } from "node:child_process";
import { copyFile, mkdir, readdir, readFile } from "node:fs/promises";
import process from "node:process";
import { fileURLToPath } from "node:url";
import { parseArgs } from "node:util";

/** The bundles of a release: two platforms, two compressions. */
const ARCHES = ["x64", "arm64"];
const COMPRESSIONS = ["gz", "xz"];

export interface Release {
  channel: "latest" | "canary";
  version: string;
}

/** Which release a built tree holds: its one meta/<channel> file. */
export async function readRelease(tree: string): Promise<Release> {
  const channels = await readdir(`${tree}/meta`);
  if (channels.length !== 1) {
    throw new Error(
      `${tree}/meta holds ${channels.join(", ")}, not one channel`,
    );
  }
  const [channel] = channels;
  if (channel !== "latest" && channel !== "canary") {
    throw new Error(`unknown channel ${channel}`);
  }
  const version = await readFile(`${tree}/meta/${channel}`, "utf8");
  const shape = channel === "latest"
    ? /^(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)$/
    : /^canary-[0-9]{4}-[0-9]{2}-[0-9]{2}-[0-9]{6}-[0-9a-f]{7,40}$/;
  if (!shape.test(version)) {
    throw new Error(
      `${channel} names ${JSON.stringify(version)}, not a ${channel} version`,
    );
  }
  return { channel, version };
}

/** The bundle file names a release has. */
export function bundles(version: string): string[] {
  return ARCHES.flatMap((arch) =>
    COMPRESSIONS.map((ext) => `dassie-${version}-linux-${arch}.tar.${ext}`)
  );
}

async function sha256(path: string): Promise<string> {
  const digest = await crypto.subtle.digest(
    "SHA-256",
    await readFile(path),
  );
  return Array.from(
    new Uint8Array(digest),
    (b) => b.toString(16).padStart(2, "0"),
  ).join("");
}

/** Checks that SHA256SUMS lists exactly the release's bundles, and that
 * each matches. */
export async function checkSums(dir: string, version: string): Promise<void> {
  const lines = (await readFile(`${dir}/SHA256SUMS`, "utf8")).trimEnd().split(
    "\n",
  );
  const listed = new Map(lines.map((line) => {
    const match = line.match(/^([0-9a-f]{64}) {2}(\S+)$/);
    if (!match) {
      throw new Error(`SHA256SUMS: malformed line ${JSON.stringify(line)}`);
    }
    return [match[2], match[1]];
  }));
  const expected = bundles(version);
  if (
    listed.size !== expected.length ||
    !expected.every((name) => listed.has(name))
  ) {
    throw new Error(
      `SHA256SUMS lists ${[...listed.keys()].join(", ")}, not ${
        expected.join(", ")
      }`,
    );
  }
  for (const [name, digest] of listed) {
    if (await sha256(`${dir}/${name}`) !== digest) {
      throw new Error(`${name} does not match SHA256SUMS`);
    }
  }
}

/** Runs `command` with `stdin`, if any, as a line on its standard input,
 * and resolves with its exit code and what it printed. */
function run(
  command: string,
  args: string[],
  stdin?: string,
): Promise<{ code: number | null; stdout: string; stderr: string }> {
  const child = spawn(command, args);
  // A command that exits without reading its input fails on its own.
  child.stdin.on("error", () => {});
  child.stdin.end(stdin === undefined ? "" : `${stdin}\n`);
  let stdout = "";
  let stderr = "";
  child.stdout.setEncoding("utf8").on("data", (text: string) => {
    stdout += text;
  });
  child.stderr.setEncoding("utf8").on("data", (text: string) => {
    stderr += text;
  });
  return new Promise((resolve, reject) => {
    child.once("error", reject);
    child.once("close", (code) => resolve({ code, stdout, stderr }));
  });
}

async function minisign(args: string[], stdin?: string): Promise<string> {
  const { code, stdout, stderr } = await run("minisign", args, stdin);
  if (code !== 0) {
    throw new Error(`minisign ${args[0]} failed: ${stderr.trim()}`);
  }
  return stdout;
}

/** Signs SHA256SUMS in `dir` for `version`. */
export async function sign(
  dir: string,
  version: string,
  secretKey: string,
  password?: string,
): Promise<void> {
  await minisign(
    [
      "-S",
      "-s",
      secretKey,
      "-m",
      `${dir}/SHA256SUMS`,
      "-x",
      `${dir}/SHA256SUMS.minisig`,
      "-t",
      `dassie ${version}`,
    ],
    password,
  );
}

/** Verifies a signed tree with `dassie-release verify`. */
export async function verify(
  tree: string,
  publicKey: string,
  verifier: string,
): Promise<void> {
  const { code, stderr } = await run(verifier, ["verify", tree, publicKey]);
  if (code !== 0) {
    throw new Error(`dassie-release verify failed: ${stderr.trim()}`);
  }
}

/** Copies the built tree to `out`, checks, signs and verifies it. */
export async function release(options: {
  tree: string;
  out: string;
  secretKey: string;
  publicKey: string;
  verifier: string;
  password?: string;
  expect?: string;
}): Promise<Release> {
  const built = await readRelease(options.tree);
  if (options.expect !== undefined && options.expect !== built.version) {
    throw new Error(`the tree holds ${built.version}, not ${options.expect}`);
  }
  await mkdir(`${options.out}/meta`, { recursive: true });
  await mkdir(`${options.out}/${built.version}`);
  await copyFile(
    `${options.tree}/meta/${built.channel}`,
    `${options.out}/meta/${built.channel}`,
  );
  for (const name of [...bundles(built.version), "SHA256SUMS"]) {
    await copyFile(
      `${options.tree}/${built.version}/${name}`,
      `${options.out}/${built.version}/${name}`,
    );
  }
  // The installer, with the release key in it, once there is one.
  await copyFile(`${options.tree}/install.sh`, `${options.out}/install.sh`)
    .catch((error: NodeJS.ErrnoException) => {
      if (error.code !== "ENOENT") throw error;
    });
  const dir = `${options.out}/${built.version}`;
  await checkSums(dir, built.version);
  await sign(dir, built.version, options.secretKey, options.password);
  await verify(options.out, options.publicKey, options.verifier);
  return built;
}

const USAGE =
  "usage: release.ts <built tree> <out dir> --secret-key <file> --public-key <file> --verifier <dassie-release> [--version <v>]";

if (process.argv[1] === fileURLToPath(import.meta.url)) {
  let args;
  try {
    args = parseArgs({
      options: {
        "secret-key": { type: "string" },
        "public-key": { type: "string" },
        verifier: { type: "string" },
        version: { type: "string" },
      },
      allowPositionals: true,
    });
  } catch (error) {
    console.error(`${(error as Error).message}\n${USAGE}`);
    process.exit(2);
  }
  const { values, positionals: [tree, out] } = args;
  if (
    !tree || !out || !values["secret-key"] || !values["public-key"] ||
    !values.verifier
  ) {
    console.error(USAGE);
    process.exit(2);
  }
  const built = await release({
    tree,
    out,
    secretKey: values["secret-key"],
    publicKey: values["public-key"],
    verifier: values.verifier,
    password: process.env.MINISIGN_PASSWORD || undefined,
    expect: values.version,
  });
  console.log(`signed dassie ${built.version} for ${built.channel} in ${out}`);
}
