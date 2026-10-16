/**
 * What the dev environment builds (spec 7 §6): the node binary with cargo,
 * again when the Rust sources change, and the wallet and the dashboard with
 * `vite build --watch` in development mode.
 *
 * @module
 */

import { type FSWatcher, watch } from "node:fs";
import { createRequire } from "node:module";
import { dirname, join } from "node:path";
import process from "node:process";
import { ChildProcess, type Exit, type Stream } from "./process.ts";
import { ROOT } from "./paths.ts";

/** How long the sources must be still before a build starts. */
export const DEBOUNCE_MS = 300;
/** The part of a failed build's output the dashboard shows. */
export const MAX_EXCERPT_BYTES = 8 * 1024;
/** How long the first Vite build may take. */
const FIRST_BUILD_TIMEOUT_MS = 180_000;

/** The binary `cargo build -p dassie-cli` makes. */
export const DEV_BINARY = `${ROOT}target/debug/dassie`;

/** Where a failed build failed: from its first error on, cut. */
export function errorExcerpt(output: string): string {
  const lines = output.split("\n");
  const first = lines.findIndex((line) => /^error\b/.test(line));
  const text = (first === -1 ? lines : lines.slice(first)).join("\n");
  return text.length > MAX_EXCERPT_BYTES
    ? `${text.slice(0, MAX_EXCERPT_BYTES)}…`
    : text;
}

export type BuildResult = { ok: true } | { ok: false; excerpt: string };

/**
 * Builds the node binary, one build at a time: a request during a build
 * runs once more after it, however many there were.
 */
export class CargoBuilder {
  #running?: Promise<BuildResult>;
  #again = false;
  readonly onStart: () => void;
  readonly onLine: (stream: Stream, line: string) => void;

  constructor(
    onStart: () => void,
    onLine: (stream: Stream, line: string) => void,
  ) {
    this.onStart = onStart;
    this.onLine = onLine;
  }

  /** Builds, or asks for one more build after the one running. */
  build(): Promise<BuildResult> {
    if (this.#running) {
      this.#again = true;
      return this.#running;
    }
    this.#running = this.#loop().finally(() => this.#running = undefined);
    return this.#running;
  }

  async #loop(): Promise<BuildResult> {
    for (;;) {
      this.#again = false;
      this.onStart();
      const result = await this.#once();
      if (!this.#again) return result;
    }
  }

  async #once(): Promise<BuildResult> {
    const child = new ChildProcess("cargo", "cargo", {
      args: ["build", "--locked", "-p", "dassie-cli"],
      cwd: ROOT,
      env: { CARGO_TERM_COLOR: "never" },
      onLine: this.onLine,
    });
    const exit: Exit = await child.exited;
    return exit.code === 0
      ? { ok: true }
      : { ok: false, excerpt: errorExcerpt(exit.output) };
  }
}

/** Calls `onChange` once the files under `paths` have been still a moment. */
export function watchSources(
  paths: readonly string[],
  onChange: (paths: string[]) => void,
): { close(): void } {
  let timer: ReturnType<typeof setTimeout> | undefined;
  let changed = new Set<string>();
  const onEvent = (dir: string, file: string | null) => {
    // Node names no file at times; the directory stands for it then.
    const path = file === null ? dir : join(dir, file);
    if (path.includes("/target/")) return;
    changed.add(path);
    clearTimeout(timer);
    timer = setTimeout(() => {
      const paths = [...changed];
      changed = new Set();
      onChange(paths);
    }, DEBOUNCE_MS);
  };
  const watchers: FSWatcher[] = paths.map((dir) =>
    watch(dir, { recursive: true }, (_, file) => onEvent(dir, file))
      // Gone, or closed: nothing more to watch.
      .on("error", () => {})
  );
  return {
    close() {
      clearTimeout(timer);
      for (const watcher of watchers) watcher.close();
    },
  };
}

/** Vite's command line script, as the app in `dir` has it installed. */
export function viteBin(dir: string): string {
  const manifest = createRequire(join(dir, "package.json"))
    .resolve("vite/package.json");
  return join(dirname(manifest), "bin", "vite.js");
}

/**
 * A development build of the app in `dir` into its `dist-dev/`, once or
 * watching. Vite's script runs in Node itself, not through a package
 * script or its `.bin` shim, so that stopping it reaches Vite.
 */
export class ViteBuild {
  #child?: ChildProcess;
  readonly dir: string;
  readonly watch: boolean;
  readonly onLine: (stream: Stream, line: string) => void;
  /** Its process runs: for the PID file. */
  readonly onSpawn: () => void;

  constructor(
    dir: string,
    watch: boolean,
    onLine: (stream: Stream, line: string) => void,
    onSpawn: () => void = () => {},
  ) {
    this.dir = dir;
    this.watch = watch;
    this.onLine = onLine;
    this.onSpawn = onSpawn;
  }

  /** Starts it; resolves once the first build is done. */
  async start(): Promise<void> {
    const args = [
      viteBin(this.dir),
      "build",
      "--configLoader",
      "native",
      "--mode",
      "development",
      ...(this.watch ? ["--watch"] : []),
    ];
    let built!: () => void;
    const first = new Promise<void>((resolve) => built = resolve);
    this.#child = new ChildProcess("vite", process.execPath, {
      args,
      cwd: this.dir,
      env: { NO_COLOR: "1" },
      onLine: (stream, line) => {
        if (/built in \d/.test(line)) built();
        this.onLine(stream, line);
      },
    });
    this.onSpawn();
    const exited = this.#child.exited.then((exit) => {
      if (exit.code !== 0) {
        throw new Error(`vite build in ${this.dir} failed:\n${exit.output}`);
      }
    });
    let timer: ReturnType<typeof setTimeout> | undefined;
    const timeout = new Promise<never>((_, reject) => {
      timer = setTimeout(
        () => reject(new Error(`vite build in ${this.dir} took too long`)),
        FIRST_BUILD_TIMEOUT_MS,
      );
    });
    try {
      await Promise.race([this.watch ? first : exited, exited, timeout]);
    } finally {
      clearTimeout(timer);
    }
  }

  async stop(): Promise<void> {
    await this.#child?.stop();
    this.#child = undefined;
  }

  /** The process, for the PID file. */
  pids(): number[] {
    return this.#child ? [this.#child.pid] : [];
  }
}
