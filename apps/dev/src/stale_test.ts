import { assertEquals } from "@std/assert";
import { recordPids, releaseLock, stopStale, takeLock } from "./stale.ts";

Deno.test("a PID now used by something else is left alone", async () => {
  const dir = await Deno.makeTempDir();
  const other = new Deno.Command("sleep", { args: ["30"] }).spawn();
  try {
    await recordPids(`${dir}/pids`, [other.pid, 999_999_999]);
    assertEquals(await stopStale(`${dir}/pids`), []);
    // Still running: killing it now works.
    other.kill("SIGKILL");
    await other.status;
    // The file is gone once read.
    assertEquals(await stopStale(`${dir}/pids`), []);
  } finally {
    await Deno.remove(dir, { recursive: true });
  }
});

Deno.test("the lock is this dev server's, unless another one holds it", async () => {
  const dir = await Deno.makeTempDir();
  try {
    // A PID that is no dev server: taken over.
    await Deno.writeTextFile(`${dir}/dev.pid`, "1\n");
    assertEquals(await takeLock(`${dir}/dev.pid`), undefined);
    assertEquals(
      (await Deno.readTextFile(`${dir}/dev.pid`)).trim(),
      `${Deno.pid}`,
    );
    // Its own: taken again.
    assertEquals(await takeLock(`${dir}/dev.pid`), undefined);
    await releaseLock(`${dir}/dev.pid`);
    assertEquals(await takeLock(`${dir}/dev.pid`), undefined);
  } finally {
    await Deno.remove(dir, { recursive: true });
  }
});
