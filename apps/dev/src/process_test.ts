import { test } from "node:test";
import { assertEquals } from "@std/assert";
import { ChildProcess, MAX_LINE_BYTES, TAIL_LINES } from "./process.ts";

test("output comes line by line, and only the tail is kept", async () => {
  const seen: string[] = [];
  const child = new ChildProcess("sh", "sh", {
    args: ["-c", `for i in $(seq 1 ${TAIL_LINES + 50}); do echo $i; done`],
    onLine: (_, line) => seen.push(line),
  });
  const { code, output } = await child.exited;
  assertEquals(code, 0);
  assertEquals(seen.length, TAIL_LINES + 50);
  const tail = output.split("\n");
  assertEquals(tail.length, TAIL_LINES);
  assertEquals(tail.at(-1), `${TAIL_LINES + 50}`);
});

test("a long line is cut", async () => {
  const child = new ChildProcess("sh", "sh", {
    args: ["-c", `head -c ${MAX_LINE_BYTES * 2} /dev/zero | tr '\\0' x`],
  });
  const { output } = await child.exited;
  assertEquals(output.length, MAX_LINE_BYTES + 1);
});

test("a process that ignores SIGTERM is killed", async () => {
  // It says "ready" once the trap is set.
  const { promise: ready, resolve } = Promise.withResolvers<void>();
  const child = new ChildProcess("stubborn", "sh", {
    args: ["-c", "trap '' TERM; echo ready; sleep 30"],
    onLine: (_, line) => {
      if (line === "ready") resolve();
    },
  });
  await ready;
  // Killed, not ended by its own sleep: 128 + SIGKILL.
  const { code } = await child.stop(200);
  assertEquals(code, 137);
});
