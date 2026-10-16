import { assertRejects } from "@std/assert";
import { once } from "node:events";
import { mkdtemp, rm, stat, writeFile } from "node:fs/promises";
import { createServer } from "node:net";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { waitForSocket } from "./wait.ts";

Deno.test("a socket file nobody answers on is not a published socket", async () => {
  const dir = await mkdtemp(join(tmpdir(), "wait-"));
  const path = `${dir}/s.sock`;
  try {
    // A socket left behind: bound, then closed without unlinking.
    const listener = createServer().listen(path);
    await once(listener, "listening");
    listener.close();
    await once(listener, "close");
    // Some systems unlink on close: a plain file stands in for the socket.
    await stat(path).catch(() => writeFile(path, ""));
    await assertRejects(
      () => waitForSocket(path, 300),
      Error,
      "did not appear",
    );
    await rm(path).catch(() => {});
    const live = createServer((socket) => socket.destroy()).listen(path);
    await once(live, "listening");
    await waitForSocket(path, 2_000);
    live.close();
    await once(live, "close");
  } finally {
    await rm(dir, { recursive: true, force: true });
  }
});
