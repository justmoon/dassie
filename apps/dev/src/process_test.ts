import { test } from "node:test";
import { assert, assertEquals } from "@std/assert";
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
  const child = new ChildProcess("stubborn", "sh", {
    args: ["-c", "trap '' TERM; echo ready; sleep 30"],
  });
  // Wait until the trap is set.
  for (let i = 0; i < 100 && child.tail() === ""; i++) {
    await new Promise((resolve) => setTimeout(resolve, 20));
  }
  const started = Date.now();
  const { code } = await child.stop(200);
  assert(Date.now() - started < 5_000);
  assertEquals(code, 137);
});
