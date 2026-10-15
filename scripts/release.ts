// Signs a release for the mirror (spec D11), as the release workflow does:
//
//   deno run -A scripts/release.ts <built tree> <out dir> \
//     --secret-key <file> --public-key resources/minisign.pub \
//     [--version <expected version>]
//
// The built tree is `nix build .#release` or `.#canary`: meta/<channel>
// and <version>/ with the bundles and SHA256SUMS. The script copies it to
// <out dir>, checks every bundle against SHA256SUMS, signs SHA256SUMS with
// minisign (SHA256SUMS.minisig, trusted comment "dassie <version>"), and
// verifies the signature with the public key the binaries are built with,
// so a release signed with any other key never reaches the mirror. The
// secret key's password, if it has one, comes from MINISIGN_PASSWORD.
//
// minisign comes from the flake's dev shell.

import { parseArgs } from "@std/cli/parse-args";

/** The bundles of a release: two platforms, two compressions. */
const ARCHES = ["x64", "arm64"];
const COMPRESSIONS = ["gz", "xz"];

export interface Release {
  channel: "latest" | "canary";
  version: string;
}

/** Which release a built tree holds: its one meta/<channel> file. */
export async function readRelease(tree: string): Promise<Release> {
  const channels = [];
  for await (const entry of Deno.readDir(`${tree}/meta`)) {
    channels.push(entry.name);
  }
  if (channels.length !== 1) {
    throw new Error(
      `${tree}/meta holds ${channels.join(", ")}, not one channel`,
    );
  }
  const [channel] = channels;
  if (channel !== "latest" && channel !== "canary") {
    throw new Error(`unknown channel ${channel}`);
  }
  const version = await Deno.readTextFile(`${tree}/meta/${channel}`);
  const shape = channel === "latest"
    ? /^(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)$/
    : /^canary-[0-9]{4}-[0-9]{2}-[0-9]{2}-[0-9a-f]{7,40}$/;
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
    await Deno.readFile(path),
  );
  return Array.from(
    new Uint8Array(digest),
    (b) => b.toString(16).padStart(2, "0"),
  ).join("");
}

/** Checks that SHA256SUMS lists exactly the release's bundles, and that
 * each matches. */
export async function checkSums(dir: string, version: string): Promise<void> {
  const lines = (await Deno.readTextFile(`${dir}/SHA256SUMS`)).trimEnd().split(
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

async function minisign(args: string[], stdin?: string): Promise<string> {
  const child = new Deno.Command("minisign", {
    args,
    stdin: stdin === undefined ? "null" : "piped",
    stdout: "piped",
    stderr: "piped",
  }).spawn();
  if (stdin !== undefined) {
    const writer = child.stdin.getWriter();
    await writer.write(new TextEncoder().encode(`${stdin}\n`));
    await writer.close();
  }
  const { success, stdout, stderr } = await child.output();
  const decoder = new TextDecoder();
  if (!success) {
    throw new Error(
      `minisign ${args[0]} failed: ${decoder.decode(stderr).trim()}`,
    );
  }
  return decoder.decode(stdout);
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

/** Verifies SHA256SUMS.minisig with `publicKey`, and that it signs the sums
 * of `version`. */
export async function verify(
  dir: string,
  version: string,
  publicKey: string,
): Promise<void> {
  const output = await minisign([
    "-V",
    "-p",
    publicKey,
    "-m",
    `${dir}/SHA256SUMS`,
    "-x",
    `${dir}/SHA256SUMS.minisig`,
  ]);
  if (!output.split("\n").includes(`Trusted comment: dassie ${version}`)) {
    throw new Error(
      `SHA256SUMS.minisig does not sign dassie ${version}:\n${output}`,
    );
  }
}

/** Copies the built tree to `out`, checks, signs and verifies it. */
export async function release(options: {
  tree: string;
  out: string;
  secretKey: string;
  publicKey: string;
  password?: string;
  expect?: string;
}): Promise<Release> {
  const built = await readRelease(options.tree);
  if (options.expect !== undefined && options.expect !== built.version) {
    throw new Error(`the tree holds ${built.version}, not ${options.expect}`);
  }
  await Deno.mkdir(`${options.out}/meta`, { recursive: true });
  await Deno.mkdir(`${options.out}/${built.version}`);
  await Deno.copyFile(
    `${options.tree}/meta/${built.channel}`,
    `${options.out}/meta/${built.channel}`,
  );
  for (const name of [...bundles(built.version), "SHA256SUMS"]) {
    await Deno.copyFile(
      `${options.tree}/${built.version}/${name}`,
      `${options.out}/${built.version}/${name}`,
    );
  }
  const dir = `${options.out}/${built.version}`;
  await checkSums(dir, built.version);
  await sign(dir, built.version, options.secretKey, options.password);
  await verify(dir, built.version, options.publicKey);
  return built;
}

if (import.meta.main) {
  const args = parseArgs(Deno.args, {
    string: ["secret-key", "public-key", "version"],
  });
  const [tree, out] = args._.map(String);
  if (!tree || !out || !args["secret-key"] || !args["public-key"]) {
    console.error(
      "usage: release.ts <built tree> <out dir> --secret-key <file> --public-key <file> [--version <v>]",
    );
    Deno.exit(2);
  }
  const built = await release({
    tree,
    out,
    secretKey: args["secret-key"],
    publicKey: args["public-key"],
    password: Deno.env.get("MINISIGN_PASSWORD") || undefined,
    expect: args.version,
  });
  console.log(`signed dassie ${built.version} for ${built.channel} in ${out}`);
}
