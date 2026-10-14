/**
 * A child process whose output is read line by line as it comes, never
 * kept whole: a long-running node would otherwise fill memory, or block on
 * a full pipe if nobody read it. The last lines are kept for reports.
 *
 * @module
 */

import { TextLineStream } from "@std/streams/text-line-stream";

/** Lines kept for {@linkcode ChildProcess.tail}. */
export const TAIL_LINES = 200;
/** Longest line kept or passed on; the rest is cut. */
export const MAX_LINE_BYTES = 4096;
/** How long a process has to exit after SIGTERM before SIGKILL. */
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
  readonly env?: Record<string, string>;
  readonly cwd?: string;
  /** Every line of output, as it comes. */
  readonly onLine?: (stream: Stream, line: string) => void;
}

export class ChildProcess {
  readonly #child: Deno.ChildProcess;
  readonly #tail: string[] = [];
  readonly #reads: Promise<void>[];
  readonly exited: Promise<Exit>;

  constructor(
    readonly name: string,
    command: string,
    options: ChildOptions = {},
  ) {
    this.#child = new Deno.Command(command, {
      args: [...(options.args ?? [])],
      env: options.env,
      cwd: options.cwd,
      stdin: "null",
      stdout: "piped",
      stderr: "piped",
    }).spawn();
    this.#reads = [
      this.#read("stdout", this.#child.stdout, options.onLine),
      this.#read("stderr", this.#child.stderr, options.onLine),
    ];
    this.exited = this.#child.status.then(async (status) => {
      await Promise.all(this.#reads);
      return { code: status.code, output: this.tail() };
    });
  }

  /** The process ID. */
  get pid(): number {
    return this.#child.pid;
  }

  /** The last lines of output. */
  tail(): string {
    return this.#tail.join("\n");
  }

  /** Stops the process: SIGTERM, and SIGKILL if it lingers. */
  async stop(graceMs = STOP_GRACE_MS): Promise<Exit> {
    signal(this.#child, "SIGTERM");
    let timer: ReturnType<typeof setTimeout> | undefined;
    const lingering = new Promise<"lingering">((resolve) => {
      timer = setTimeout(() => resolve("lingering"), graceMs);
    });
    const first = await Promise.race([this.exited, lingering]);
    clearTimeout(timer);
    if (first === "lingering") signal(this.#child, "SIGKILL");
    return await this.exited;
  }

  async #read(
    stream: Stream,
    readable: ReadableStream<Uint8Array<ArrayBuffer>>,
    onLine: ChildOptions["onLine"],
  ): Promise<void> {
    const lines = readable
      .pipeThrough(new TextDecoderStream())
      .pipeThrough(new TextLineStream());
    try {
      for await (const whole of lines) {
        const line = cut(whole);
        this.#tail.push(line);
        if (this.#tail.length > TAIL_LINES) this.#tail.shift();
        onLine?.(stream, line);
      }
    } catch {
      // The pipe broke: the process is gone.
    }
  }
}

function signal(child: Deno.ChildProcess, name: Deno.Signal): void {
  try {
    child.kill(name);
  } catch {
    // It exited already.
  }
}

/** `line`, cut at {@linkcode MAX_LINE_BYTES} characters. */
export function cut(line: string): string {
  return line.length > MAX_LINE_BYTES
    ? `${line.slice(0, MAX_LINE_BYTES)}…`
    : line;
}
