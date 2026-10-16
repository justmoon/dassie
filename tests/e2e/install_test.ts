/**
 * The M8 done criterion (spec 7 §8): the installer and the updater work on
 * Debian, Ubuntu, Fedora and CentOS, and a tampered bundle is refused.
 *
 * Each distribution runs in a container with systemd as PID 1
 * (`install/<distro>.Dockerfile`, privileged). A mirror, Caddy on a Docker
 * network of the test's own, serves `https://mirror.test` under a
 * certificate from a CA the container is made to trust. It holds the
 * releases of `nix build .#install-test-mirror`: 1.0.0 and 1.0.1 for x64,
 * trusting the test key and signed with it, and install.sh with the test
 * key. In each container:
 *
 * 1. install.sh refuses a tampered bundle, and sums signed for another
 *    version, and installs nothing;
 * 2. it installs 1.0.0: the node runs under systemd, socket-activated,
 *    and answers on port 443;
 * 3. the updater, as its timer starts it, refuses 1.0.1 tampered, or with
 *    the sums of 1.0.0, and the node runs on as it was;
 * 4. it installs 1.0.1, restarts the node, and keeps its data;
 * 5. install.sh uninstalls it.
 *
 * Set `DASSIE_INSTALL_MIRROR` to the built mirror to run it; it needs
 * Docker, which may run privileged containers, and mkcert.
 * `DASSIE_INSTALL_DISTROS` picks distributions, by default all four. The
 * release workflow runs one job per distribution before it signs a release.
 */

import { test, type TestContext } from "node:test";
import { Buffer } from "node:buffer";
import { spawn } from "node:child_process";
import { mkdir, mkdtemp, readFile, writeFile } from "node:fs/promises";
import { constants, tmpdir } from "node:os";
import { join } from "node:path";
import process from "node:process";

import { assert, assertEquals, assertStringIncludes } from "@std/assert";

const MIRROR = process.env.DASSIE_INSTALL_MIRROR;
const DISTROS =
  (process.env.DASSIE_INSTALL_DISTROS ?? "debian,ubuntu,fedora,centos")
    .split(",")
    .filter((distro) => distro !== "");
const IMAGES = new URL("./install/", import.meta.url).pathname;
/** Caddy, which serves the mirror. */
const CADDY =
  "caddy:2-alpine@sha256:d8542f48d34a9cf4e4c11a478865229840e87e4c96ea3f439101f31a5d35f75f";
/** Where each distribution trusts a CA from, and how it takes it in. */
const TRUST: Record<string, [string, string]> = {
  debian: [
    "/usr/local/share/ca-certificates/mirror-test.crt",
    "update-ca-certificates",
  ],
  ubuntu: [
    "/usr/local/share/ca-certificates/mirror-test.crt",
    "update-ca-certificates",
  ],
  fedora: [
    "/etc/pki/ca-trust/source/anchors/mirror-test.pem",
    "update-ca-trust",
  ],
  centos: [
    "/etc/pki/ca-trust/source/anchors/mirror-test.pem",
    "update-ca-trust",
  ],
};

interface Run {
  code: number;
  stdout: string;
  stderr: string;
}

function run(
  command: string,
  args: string[],
  env?: Record<string, string>,
): Promise<Run> {
  const child = spawn(command, args, {
    env: { ...process.env, ...env },
    stdio: ["ignore", "pipe", "pipe"],
  });
  const stdout: Buffer[] = [];
  const stderr: Buffer[] = [];
  child.stdout.on("data", (chunk: Buffer) => stdout.push(chunk));
  child.stderr.on("data", (chunk: Buffer) => stderr.push(chunk));
  return new Promise((resolve, reject) => {
    child.once("error", reject);
    child.once("close", (code, signal) =>
      resolve({
        code: code ?? 128 + constants.signals[signal!],
        stdout: Buffer.concat(stdout).toString(),
        stderr: Buffer.concat(stderr).toString(),
      }));
  });
}

async function must(
  command: string,
  args: string[],
  env?: Record<string, string>,
): Promise<string> {
  const result = await run(command, args, env);
  if (result.code !== 0) {
    throw new Error(
      `${command} ${
        args.join(" ")
      } failed (${result.code}):\n${result.stdout}${result.stderr}`,
    );
  }
  return result.stdout;
}

/** The mirror: a copy of the built one the tests can tamper with, served
 * by Caddy as `https://mirror.test` on a Docker network of its own, which
 * the machines join. Nothing goes through the host's network. */
class Mirror {
  readonly url = "https://mirror.test";
  readonly root: string;
  readonly ca: string;
  readonly network: string;
  readonly #server: string;

  private constructor(
    root: string,
    ca: string,
    network: string,
    server: string,
  ) {
    this.root = root;
    this.ca = ca;
    this.network = network;
    this.#server = server;
  }

  static async start(built: string): Promise<Mirror> {
    const work = await mkdtemp(join(tmpdir(), "dassie-install-"));
    const root = `${work}/www`;
    await must("cp", ["-rL", "--no-preserve=mode", built, root]);
    const caroot = `${work}/ca`;
    await mkdir(caroot);
    await mkdir(`${work}/certs`);
    await must("mkcert", [
      "-cert-file",
      `${work}/certs/cert.pem`,
      "-key-file",
      `${work}/certs/key.pem`,
      "mirror.test",
    ], { CAROOT: caroot });
    await writeFile(
      `${work}/Caddyfile`,
      "{\n  admin off\n  auto_https disable_redirects\n}\n" +
        "https://mirror.test {\n  tls /certs/cert.pem /certs/key.pem\n" +
        "  root * /srv\n  file_server\n}\n",
    );
    const network = `dassie-install-${crypto.randomUUID().slice(0, 8)}`;
    await must("docker", ["network", "create", network]);
    const server = (await must("docker", [
      "run",
      "-d",
      "--network",
      network,
      "--network-alias",
      "mirror.test",
      "-v",
      `${root}:/srv:ro`,
      "-v",
      `${work}/certs:/certs:ro`,
      "-v",
      `${work}/Caddyfile:/etc/caddy/Caddyfile:ro`,
      CADDY,
    ])).trim();
    return new Mirror(root, `${caroot}/rootCA.pem`, network, server);
  }

  /** Names `version` the newest on `latest`. */
  async offer(version: string): Promise<void> {
    await mkdir(`${this.root}/meta`, { recursive: true });
    await writeFile(`${this.root}/meta/latest`, version);
  }

  /** Runs `test` with a file of the mirror replaced, then puts it back. */
  async with<T>(
    path: string,
    content: Uint8Array,
    test: () => Promise<T>,
  ): Promise<T> {
    const original = await readFile(`${this.root}/${path}`);
    await writeFile(`${this.root}/${path}`, content);
    try {
      return await test();
    } finally {
      await writeFile(`${this.root}/${path}`, original);
    }
  }

  async file(path: string): Promise<Uint8Array> {
    // A copy, not a Buffer: a Buffer's slice() shares its bytes.
    return new Uint8Array(await readFile(`${this.root}/${path}`));
  }

  async stop(): Promise<void> {
    await run("docker", ["rm", "-f", this.#server]);
    await run("docker", ["network", "rm", this.network]);
  }
}

/** A distribution in a container, with systemd up and the mirror's CA
 * trusted. */
class Machine {
  readonly distro: string;
  readonly id: string;

  private constructor(distro: string, id: string) {
    this.distro = distro;
    this.id = id;
  }

  static async boot(distro: string, mirror: Mirror): Promise<Machine> {
    const image = `dassie-install-test-${distro}`;
    await must("docker", [
      "build",
      "-q",
      "-t",
      image,
      "-f",
      `${IMAGES}${distro}.Dockerfile`,
      IMAGES,
    ]);
    const id = (await must("docker", [
      "run",
      "-d",
      "--privileged",
      "--network",
      mirror.network,
      image,
    ])).trim();
    const machine = new Machine(distro, id);
    await machine.until("systemd is up", async () => {
      const state = await machine.sh("systemctl is-system-running --wait");
      return ["running", "degraded"].includes(state.stdout.trim());
    });
    const [path, update] = TRUST[distro];
    await must("docker", ["cp", mirror.ca, `${id}:${path}`]);
    await machine.must(update);
    await machine.until("the mirror answers", async () => {
      const url = `${mirror.url}/install.sh`;
      return (await machine.sh(machine.fetch(url, "/root/install.sh")))
        .code === 0;
    });
    return machine;
  }

  sh(script: string): Promise<Run> {
    return run("docker", ["exec", this.id, "sh", "-c", script]);
  }

  async must(script: string): Promise<string> {
    const result = await this.sh(script);
    if (result.code !== 0) {
      throw new Error(
        `${this.distro}: ${script} failed (${result.code}):\n${result.stdout}${result.stderr}`,
      );
    }
    return result.stdout.trim();
  }

  /** A script that fetches `url` into `file`, with curl, or with wget where
   * there is no curl. */
  fetch(url: string, file: string): string {
    return `if command -v curl > /dev/null; then curl --proto =https -sSf -o ${file} ${url}; ` +
      `else wget -q -O ${file} ${url}; fi`;
  }

  /** Runs install.sh from the mirror, as `curl … | sh` would, under the
   * umask of a hardened host. */
  installer(mirror: Mirror, args = ""): Promise<Run> {
    return this.sh(
      `${this.fetch(`${mirror.url}/install.sh`, "/root/install.sh")} && ` +
        `umask 077 && sh /root/install.sh --mirror ${mirror.url} ${args}`,
    );
  }

  async until(
    what: string,
    check: () => Promise<boolean>,
    ms = 60_000,
  ): Promise<void> {
    const deadline = Date.now() + ms;
    while (Date.now() < deadline) {
      if (await check().catch(() => false)) return;
      await new Promise((resolve) => setTimeout(resolve, 500));
    }
    throw new Error(
      `${this.distro}: ${what} did not happen within ${ms / 1000} s`,
    );
  }

  async stop(): Promise<void> {
    await run("docker", ["rm", "-f", this.id]);
  }
}

function tampered(bytes: Uint8Array): Uint8Array {
  const copy = bytes.slice();
  copy[copy.length - 1] ^= 1;
  return copy;
}

/** What the node shows of itself: its version, the version `current`
 * names, whether it runs, and since when. */
async function node(machine: Machine) {
  return {
    version: await machine.must("dassie --version"),
    current: await machine.must("readlink /opt/dassie/current"),
    active: await machine.must("systemctl is-active dassie.service"),
    since: await machine.must(
      "systemctl show -p ActiveEnterTimestampMonotonic --value dassie.service",
    ),
  };
}

/** The node answers HTTPS on port 443, through its socket unit. */
async function answers(machine: Machine): Promise<void> {
  await machine.until("the node answers on port 443", async () => {
    const result = await machine.sh(
      "if command -v curl > /dev/null; then " +
        "curl -sk -o /dev/null -w '%{http_code}' https://localhost/; " +
        "else wget -q -O /dev/null --no-check-certificate https://localhost/ && echo 200; fi",
    );
    return result.stdout.trim() === "200";
  });
}

/** Starts the updater as its timer would, and returns what it said. */
async function update(machine: Machine): Promise<Run> {
  const result = await machine.sh("systemctl start dassie-update.service");
  const journal = await machine.sh(
    "journalctl -u dassie-update.service --no-pager -o cat -n 30",
  );
  await machine.sh("systemctl reset-failed dassie-update.service");
  return { ...result, stdout: journal.stdout };
}

for (const distro of DISTROS) {
  test(`install, update and uninstall on ${distro}`, {
    skip: MIRROR === undefined,
  }, async (t) => {
    const mirror = await Mirror.start(MIRROR!);
    let machine: Machine | undefined;
    try {
      machine = await Machine.boot(distro, mirror);
      await steps(t, machine, mirror);
    } finally {
      await machine?.stop();
      await mirror.stop();
    }
  });
}

async function steps(t: TestContext, machine: Machine, mirror: Mirror) {
  await mirror.offer("1.0.0");
  // The installer takes xz where there is xz, else gzip; tamper with both.
  const bundles = [
    "dassie-1.0.0-linux-x64.tar.xz",
    "dassie-1.0.0-linux-x64.tar.gz",
  ];

  await t.test("the installer refuses a tampered bundle", async () => {
    const [xz, gz] = await Promise.all(
      bundles.map((name) => mirror.file(`1.0.0/${name}`)),
    );
    const result = await mirror.with(
      `1.0.0/${bundles[0]}`,
      tampered(xz),
      () =>
        mirror.with(
          `1.0.0/${bundles[1]}`,
          tampered(gz),
          () => machine.installer(mirror),
        ),
    );
    assert(result.code !== 0, result.stdout);
    assertStringIncludes(result.stderr, "E_BUNDLE_VERIFY_FAILED");
    assertEquals((await machine.sh("test -e /opt/dassie")).code, 1);
  });

  await t.test(
    "the installer refuses sums that are not what was signed",
    async () => {
      const sums = new TextDecoder().decode(
        await mirror.file("1.0.0/SHA256SUMS"),
      );
      const changed = (sums[0] === "0" ? "1" : "0") + sums.slice(1);
      const result = await mirror.with(
        "1.0.0/SHA256SUMS",
        new TextEncoder().encode(changed),
        () => machine.installer(mirror),
      );
      assert(result.code !== 0, result.stdout);
      assertStringIncludes(result.stderr, "E_SIGNATURE_INVALID");
      assertEquals((await machine.sh("test -e /opt/dassie")).code, 1);
    },
  );

  await t.test(
    "the installer refuses sums signed for another version",
    async () => {
      const [sums, signature] = await Promise.all(
        ["1.0.1/SHA256SUMS", "1.0.1/SHA256SUMS.minisig"].map((path) =>
          mirror.file(path)
        ),
      );
      const result = await mirror.with(
        "1.0.0/SHA256SUMS",
        sums,
        () =>
          mirror.with(
            "1.0.0/SHA256SUMS.minisig",
            signature,
            () => machine.installer(mirror),
          ),
      );
      assert(result.code !== 0, result.stdout);
      assertStringIncludes(result.stderr, "E_SIGNATURE_INVALID");
      assertStringIncludes(result.stderr, "signed for another version");
      assertEquals((await machine.sh("test -e /opt/dassie")).code, 1);
    },
  );

  await t.test("the installer installs 1.0.0, and the node runs", async () => {
    const result = await machine.installer(mirror);
    assertEquals(result.code, 0, `${result.stdout}${result.stderr}`);
    assertStringIncludes(result.stdout, "Dassie 1.0.0 is installed");
    const now = await node(machine);
    assertEquals([now.version, now.current, now.active], [
      "dassie 1.0.0",
      "1.0.0",
      "active",
    ]);
    await answers(machine);
    // The node keeps its data where systemd gives it a directory.
    await machine.must("test -s /var/lib/private/dassie/dassie.sqlite3");
    // The updater looks at the test mirror, as an owner may set it to.
    await machine.must(
      "mkdir -p /etc/systemd/system/dassie-update.service.d && " +
        "printf '[Service]\\nEnvironment=DASSIE_MIRROR_URL=" + mirror.url +
        "\\n' > /etc/systemd/system/dassie-update.service.d/mirror.conf && " +
        "systemctl daemon-reload",
    );
    assertEquals(
      await machine.must("systemctl is-enabled dassie-update.timer"),
      "enabled",
    );
  });

  const before = await node(machine);
  await mirror.offer("1.0.1");

  await t.test(
    "the updater refuses a tampered bundle, and the node runs on",
    async () => {
      const name = "1.0.1/dassie-1.0.1-linux-x64.tar.gz";
      const result = await mirror.with(
        name,
        tampered(await mirror.file(name)),
        () => update(machine),
      );
      assert(result.code !== 0, result.stdout);
      assertStringIncludes(result.stdout, "does not match its signed sum");
      assertEquals(await node(machine), before);
    },
  );

  await t.test("the updater refuses the sums of another version", async () => {
    const [sums, signature] = await Promise.all(
      ["1.0.0/SHA256SUMS", "1.0.0/SHA256SUMS.minisig"].map((path) =>
        mirror.file(path)
      ),
    );
    const result = await mirror.with(
      "1.0.1/SHA256SUMS",
      sums,
      () =>
        mirror.with(
          "1.0.1/SHA256SUMS.minisig",
          signature,
          () => update(machine),
        ),
    );
    assert(result.code !== 0, result.stdout);
    assertStringIncludes(result.stdout, "signed for another version");
    assertEquals(await node(machine), before);
  });

  await t.test(
    "the updater installs 1.0.1, restarts the node, and keeps its data",
    async () => {
      const database = await machine.must(
        "stat -c %i /var/lib/private/dassie/dassie.sqlite3",
      );
      const result = await update(machine);
      assertEquals(result.code, 0, result.stdout);
      assertStringIncludes(result.stdout, "Installed dassie 1.0.1.");
      const after = await node(machine);
      assertEquals([after.version, after.current, after.active], [
        "dassie 1.0.1",
        "1.0.1",
        "active",
      ]);
      assert(after.since !== before.since, "the node was not restarted");
      await answers(machine);
      assertEquals(
        await machine.must("stat -c %i /var/lib/private/dassie/dassie.sqlite3"),
        database,
      );
      // The version it came from stays, beside the record for
      // `dassie status`.
      assertEquals(
        await machine.must("ls /opt/dassie"),
        "1.0.0\n1.0.1\ncurrent\nlast-update",
      );
      assertStringIncludes(
        await machine.must("cat /opt/dassie/last-update"),
        "outcome=updated\ntarget=1.0.1",
      );
      // Nothing newer: the updater stays.
      const again = await update(machine);
      assertEquals(again.code, 0, again.stdout);
      assertStringIncludes(again.stdout, "Already up to date (1.0.1).");
    },
  );

  await t.test(
    "the installer does not go back to what the mirror offers",
    async () => {
      await mirror.offer("1.0.0");
      const result = await machine.installer(mirror);
      assert(result.code !== 0, result.stdout);
      assertStringIncludes(result.stderr, "E_VERSION_OLDER");
      assertEquals((await node(machine)).version, "dassie 1.0.1");
    },
  );

  await t.test("the installer uninstalls it", async () => {
    const result = await machine.installer(mirror, "uninstall");
    assertEquals(result.code, 0, `${result.stdout}${result.stderr}`);
    assertEquals(
      (await machine.sh("test -e /opt/dassie || test -e /usr/local/bin/dassie"))
        .code,
      1,
    );
    for (
      const unit of [
        "dassie.service",
        "dassie-http.socket",
        "dassie-https.socket",
        "dassie-ipc.socket",
        "dassie-update.service",
        "dassie-update.timer",
      ]
    ) {
      await machine.must(`test ! -e /etc/systemd/system/${unit}`);
    }
    assertEquals(
      await machine.must("systemctl is-active dassie.service || true"),
      "inactive",
    );
  });
}
