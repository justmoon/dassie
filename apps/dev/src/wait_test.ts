import { assertRejects } from "@std/assert";
import { waitForSocket } from "./wait.ts";

Deno.test("a socket file nobody answers on is not a published socket", async () => {
  const dir = await Deno.makeTempDir();
  const path = `${dir}/s.sock`;
  try {
    // A socket left behind: bound, then closed without unlinking.
    const listener = Deno.listen({ transport: "unix", path });
    listener.close();
    // Some systems unlink on close: a plain file stands in for the socket.
    await Deno.stat(path).catch(() => Deno.writeTextFile(path, ""));
    await assertRejects(
      () => waitForSocket(path, 300),
      Error,
      "did not appear",
    );
    await Deno.remove(path).catch(() => {});
    const live = Deno.listen({ transport: "unix", path });
    const accepting = live.accept().then((conn) => conn.close(), () => {});
    await waitForSocket(path, 2_000);
    live.close();
    await accepting;
  } finally {
    await Deno.remove(dir, { recursive: true });
  }
});
