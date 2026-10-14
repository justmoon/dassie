/**
 * The processes a dev server started, written down as they change, so that
 * the next one can stop them if this one crashed: a node left running holds
 * its ports and its socket ("another daemon is listening").
 *
 * @module
 */

/** Writes the PIDs of the running children. */
export async function recordPids(
  file: string,
  pids: readonly number[],
): Promise<void> {
  await Deno.writeTextFile(file, pids.map((pid) => `${pid}\n`).join(""));
}

/** What a process runs, or nothing if it is gone. */
async function commandOf(pid: number): Promise<string | undefined> {
  if (Deno.build.os === "linux") {
    try {
      const bytes = await Deno.readFile(`/proc/${pid}/cmdline`);
      return new TextDecoder().decode(bytes).replaceAll("\0", " ");
    } catch {
      return undefined;
    }
  }
  const output = await new Deno.Command("ps", {
    args: ["-o", "command=", "-p", `${pid}`],
    stdout: "piped",
    stderr: "null",
  }).output();
  const text = new TextDecoder().decode(output.stdout).trim();
  return output.success && text !== "" ? text : undefined;
}

/**
 * Stops the processes in `file` that are still a dassie node or a Vite
 * build; a PID used again by something else is left alone. Their names.
 */
export async function stopStale(file: string): Promise<string[]> {
  let text: string;
  try {
    text = await Deno.readTextFile(file);
  } catch {
    return [];
  }
  const stopped: string[] = [];
  for (const line of text.split("\n")) {
    const pid = Number(line.trim());
    if (!Number.isSafeInteger(pid) || pid <= 1 || pid === Deno.pid) continue;
    const command = await commandOf(pid);
    if (!command || !/\bdassie daemon\b|\bvite\b/.test(command)) continue;
    try {
      Deno.kill(pid, "SIGKILL");
      stopped.push(`${pid} (${command.slice(0, 80)})`);
    } catch {
      // Gone in between.
    }
  }
  await Deno.remove(file).catch(() => {});
  return stopped;
}
