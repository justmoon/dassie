import { assertEquals, assertMatch, assertThrows } from "@std/assert";
import {
  checkSocketPath,
  layout,
  MAX_SOCKET_PATH_BYTES,
  SOCKET_SUFFIX_BYTES,
  socketPath,
} from "./paths.ts";

Deno.test("a socket path leaves room for the node's suffix", () => {
  const fits = "/" +
    "s".repeat(MAX_SOCKET_PATH_BYTES - SOCKET_SUFFIX_BYTES - 1);
  checkSocketPath(fits);
  assertThrows(() => checkSocketPath(`${fits}x`), Error, "too long");
});

Deno.test("the sockets' directory is short, and the same for the same state", async () => {
  const a = await layout("/some/where/deep/inside/a/checkout/local/dev");
  const b = await layout("/some/where/deep/inside/a/checkout/local/dev");
  const c = await layout("/elsewhere");
  assertEquals(a.sockets, b.sockets);
  assertMatch(a.sockets, /\/dassie-dev-[0-9a-f]{12}$/);
  assertEquals(a.sockets === c.sockets, false);
  assertMatch(socketPath(a, "d100"), /\/d100\.sock$/);
});
