/**
 * The processes a dev server started, written down as they change, so that
 * the next one can stop them if this one crashed: a node left running holds
 * its ports and its socket ("another daemon is listening").
 *
 * @module
 */

import { readFile, rm, writeFile } from "node:fs/promises";
import process from "node:process";
import { run } from "./process.ts";

/** Writes the PIDs of the running children. */
export async function recordPids(
  file: string,
  pids: readonly number[],
): Promise<void> {
  await writeFile(file, pids.map((pid) => `${pid}\n`).join(""));
}

/** What a process runs, or nothing if it is gone. */
async function commandOf(pid: number): Promise<string | undefined> {
  if (process.platform === "linux") {
    try {
      const bytes = await readFile(`/proc/${pid}/cmdline`);
      return new TextDecoder().decode(bytes).replaceAll("\0", " ");
    } catch {
      return undefined;
    }
  }
  const output = await run("ps", ["-o", "command=", "-p", `${pid}`]);
  const text = new TextDecoder().decode(output.stdout).trim();
  return output.success && text !== "" ? text : undefined;
}

/**
 * Takes the state directory for this dev server: refuses, with the PID of
 * the other one, while another dev server runs on it, since stopping its
 * children would pull its network from under it.
 */
export async function takeLock(file: string): Promise<number | undefined> {
  const other = Number((await readFile(file, "utf8").catch(() => "")).trim());
  if (Number.isSafeInteger(other) && other > 1 && other !== process.pid) {
    const command = await commandOf(other);
    if (command && /apps\/dev\/main\.ts/.test(command)) return other;
  }
  await writeFile(file, `${process.pid}\n`);
  return undefined;
}

/** Gives the state directory up. */
export async function releaseLock(file: string): Promise<void> {
  await rm(file).catch(() => {});
}

/**
 * Stops the processes in `file` that are still a dassie node or a Vite
 * build; a PID used again by something else is left alone. Their names.
 */
export async function stopStale(file: string): Promise<string[]> {
  let text: string;
  try {
    text = await readFile(file, "utf8");
  } catch {
    return [];
  }
  const stopped: string[] = [];
  for (const line of text.split("\n")) {
    const pid = Number(line.trim());
    if (!Number.isSafeInteger(pid) || pid <= 1 || pid === process.pid) continue;
    const command = await commandOf(pid);
    if (!command || !/\bdassie daemon\b|\bvite\b/.test(command)) continue;
    try {
      process.kill(pid, "SIGKILL");
      stopped.push(`${pid} (${command.slice(0, 80)})`);
    } catch {
      // Gone in between.
    }
  }
  await rm(file).catch(() => {});
  return stopped;
}
