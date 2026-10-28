import { test } from "node:test";
import { assertEquals } from "@std/assert";
import { recent } from "./recent.ts";

const at = (id: string, createdAtUnixMs: bigint) => ({ id, createdAtUnixMs });

test("recent payments are both lists, newest first, cut to the count", () => {
  const list = recent(
    [at("a", 1n), at("b", 4n)],
    [at("c", 3n), at("d", 2n)],
    3,
  );
  assertEquals(list.map((item) => item.key), ["in-b", "out-c", "out-d"]);
  assertEquals(list.map((item) => item.direction), ["in", "out", "out"]);
});

test("payments made at once keep one order", () => {
  const list = recent([at("x", 5n)], [at("x", 5n), at("a", 5n)], 5);
  assertEquals(list.map((item) => item.key), ["in-x", "out-a", "out-x"]);
});
