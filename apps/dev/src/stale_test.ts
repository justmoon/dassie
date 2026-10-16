import { assertEquals } from "@std/assert";
import { spawn } from "node:child_process";
import { once } from "node:events";
import { mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import process from "node:process";
import { recordPids, releaseLock, stopStale, takeLock } from "./stale.ts";

Deno.test("a PID now used by something else is left alone", async () => {
  const dir = await mkdtemp(join(tmpdir(), "stale-"));
  const other = spawn("sleep", ["30"]);
  const closed = once(other, "close");
  try {
    await recordPids(`${dir}/pids`, [other.pid!, 999_999_999]);
    assertEquals(await stopStale(`${dir}/pids`), []);
    // Still running: killing it now works.
    assertEquals(other.kill("SIGKILL"), true);
    await closed;
    // The file is gone once read.
    assertEquals(await stopStale(`${dir}/pids`), []);
  } finally {
    await rm(dir, { recursive: true, force: true });
  }
});

Deno.test("the lock is this dev server's, unless another one holds it", async () => {
  const dir = await mkdtemp(join(tmpdir(), "stale-"));
  try {
    // A PID that is no dev server: taken over.
    await writeFile(`${dir}/dev.pid`, "1\n");
    assertEquals(await takeLock(`${dir}/dev.pid`), undefined);
    assertEquals(
      (await readFile(`${dir}/dev.pid`, "utf8")).trim(),
      `${process.pid}`,
    );
    // Its own: taken again.
    assertEquals(await takeLock(`${dir}/dev.pid`), undefined);
    await releaseLock(`${dir}/dev.pid`);
    assertEquals(await takeLock(`${dir}/dev.pid`), undefined);
  } finally {
    await rm(dir, { recursive: true, force: true });
  }
});
