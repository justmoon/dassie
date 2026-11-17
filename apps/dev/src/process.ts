/**
 * A child process whose output is read line by line as it comes, never
 * kept whole: a long-running node would otherwise fill memory, or block on
 * a full pipe if nobody read it. The last lines are kept for reports.
 *
 * @module
 */

import {
  type ChildProcess as NodeChildProcess,
  spawn,
} from "node:child_process";
import { constants } from "node:os";
import process from "node:process";
import { createInterface } from "node:readline";
import type { Readable } from "node:stream";

/** Lines kept for {@linkcode ChildProcess.tail}. */
export const TAIL_LINES = 200;
/** Longest line kept or passed on; the rest is cut. */
export const MAX_LINE_BYTES = 4096;
/**
 * How long a process has to exit after SIGTERM before a second SIGTERM,
 * and after that before SIGKILL.
 */
export const STOP_GRACE_MS = 5_000;

export type Stream = "stdout" | "stderr";

/** How a process ended. */
export interface Exit {
  readonly code: number;
  /** Its last lines of output, both streams interleaved. */
  readonly output: string;
}

export interface ChildOptions {
  readonly args?: readonly string[];
  /** Added to this process's environment. */
  readonly env?: Record<string, string>;
  readonly cwd?: string;
  /** Every line of output, as it comes. */
  readonly onLine?: (stream: Stream, line: string) => void;
}

export class ChildProcess {
  readonly #child: NodeChildProcess;
  readonly #pid: number;
  readonly #tail: string[] = [];
  readonly exited: Promise<Exit>;
  readonly name: string;

  /** Throws if `command` cannot be run. */
  constructor(name: string, command: string, options: ChildOptions = {}) {
    this.name = name;
    this.#child = spawn(command, [...(options.args ?? [])], {
      env: { ...process.env, ...options.env },
      cwd: options.cwd,
      stdio: ["ignore", "pipe", "pipe"],
    });
    // A command that cannot start has no PID, and errs only later.
    this.#child.on("error", () => {});
    if (this.#child.pid === undefined) {
      throw new Error(`cannot run ${command}`);
    }
    this.#pid = this.#child.pid;
    const closed = new Promise<number>((resolve) => {
      this.#child.once("close", (code, signal) => {
        resolve(exitCode(code, signal));
      });
    });
    const reads = [
      this.#read("stdout", this.#child.stdout!, options.onLine),
      this.#read("stderr", this.#child.stderr!, options.onLine),
    ];
    this.exited = closed.then(async (code) => {
      await Promise.all(reads);
      return { code, output: this.tail() };
    });
  }

  /** The process ID. */
  get pid(): number {
    return this.#pid;
  }

  /** The last lines of output. */
  tail(): string {
    return this.#tail.join("\n");
  }

  /**
   * Stops the process: SIGTERM; a second if it lingers, which stops a node
   * still draining packets in flight at once, as their answers may never
   * come when its peers stop with it
   * (spec/decisions/drain-on-shutdown.md); and SIGKILL if it lingers still.
   */
  async stop(graceMs = STOP_GRACE_MS): Promise<Exit> {
    for (const signal of ["SIGTERM", "SIGTERM", "SIGKILL"] as const) {
      this.#child.kill(signal);
      if (signal === "SIGKILL" || await this.#exitsWithin(graceMs)) break;
    }
    return await this.exited;
  }

  /** Whether the process exits within `ms`. */
  async #exitsWithin(ms: number): Promise<boolean> {
    let timer: ReturnType<typeof setTimeout> | undefined;
    const lingering = new Promise<false>((resolve) => {
      timer = setTimeout(() => resolve(false), ms);
    });
    const exited = this.exited.then(() => true);
    const first = await Promise.race([exited, lingering]);
    clearTimeout(timer);
    return first;
  }

  /** Resolves once `readable` has ended, or broke with the process. */
  #read(
    stream: Stream,
    readable: Readable,
    onLine: ChildOptions["onLine"],
  ): Promise<void> {
    const lines = createInterface({ input: readable, crlfDelay: Infinity });
    lines.on("line", (whole: string) => {
      const line = cut(whole);
      this.#tail.push(line);
      if (this.#tail.length > TAIL_LINES) this.#tail.shift();
      onLine?.(stream, line);
    });
    return new Promise((resolve) => {
      // The pipe broke: the process is gone.
      lines.on("error", () => resolve());
      readable.once("close", () => resolve());
    });
  }
}

/**
 * A process's exit code from Node's `close` or `exit` event: for a process
 * a signal ended, 128 plus the signal's number, as a shell gives it.
 */
export function exitCode(
  code: number | null,
  signal: NodeJS.Signals | null,
): number {
  if (code !== null) return code;
  return signal === null ? 1 : 128 + constants.signals[signal];
}

/** What a process that ran to its end printed. */
export interface Output {
  readonly code: number;
  readonly success: boolean;
  readonly stdout: Uint8Array;
  readonly stderr: Uint8Array;
}

export interface RunOptions {
  /** Added to this process's environment. */
  readonly env?: Record<string, string>;
  readonly cwd?: string;
}

/**
 * Runs `command` to its end and collects its output, whatever its exit
 * code. Rejects only if it cannot be run.
 */
export function run(
  command: string,
  args: readonly string[],
  options: RunOptions = {},
): Promise<Output> {
  return new Promise((resolve, reject) => {
    const child = spawn(command, [...args], {
      env: { ...process.env, ...options.env },
      cwd: options.cwd,
      stdio: ["ignore", "pipe", "pipe"],
    });
    const stdout: Uint8Array[] = [];
    const stderr: Uint8Array[] = [];
    child.stdout.on("data", (chunk: Uint8Array) => stdout.push(chunk));
    child.stderr.on("data", (chunk: Uint8Array) => stderr.push(chunk));
    // A command that cannot start errs, and may close after that too.
    child.once("error", reject);
    child.once("close", (code, signal) => {
      const exit = exitCode(code, signal);
      resolve({
        code: exit,
        success: exit === 0,
        stdout: concat(stdout),
        stderr: concat(stderr),
      });
    });
  });
}

function concat(chunks: readonly Uint8Array[]): Uint8Array {
  const whole = new Uint8Array(
    chunks.reduce((length, chunk) => length + chunk.length, 0),
  );
  let offset = 0;
  for (const chunk of chunks) {
    whole.set(chunk, offset);
    offset += chunk.length;
  }
  return whole;
}

/** `line`, cut at {@linkcode MAX_LINE_BYTES} characters. */
export function cut(line: string): string {
  return line.length > MAX_LINE_BYTES
    ? `${line.slice(0, MAX_LINE_BYTES)}…`
    : line;
}
