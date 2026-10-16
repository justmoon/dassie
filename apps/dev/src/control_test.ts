import { test } from "node:test";
import { assert, assertEquals } from "@std/assert";
import { create, toBinary } from "@bufbuild/protobuf";
import {
  MAX_CONTROL_MESSAGE_BYTES,
  SourcedLogLineSchema,
} from "@dassie/rpc/dev";
import { chunkLines, ControlHub, type ControlSource } from "./control.ts";
import type { Socket } from "./serve.ts";

const line = (n: number, bytes = 100) =>
  create(SourcedLogLineSchema, {
    source: "d1",
    line: { index: BigInt(n), message: "x".repeat(bytes) },
  });

test("log lines go out in chunks that fit a message", () => {
  const lines = Array.from({ length: 5_000 }, (_, n) => line(n, 900));
  const chunks = chunkLines(lines, 64 * 1024);
  assertEquals(chunks.flat().length, lines.length);
  assertEquals(
    chunks.flat().map((l) => l.line!.index),
    lines.map((l) => l.line!.index),
  );
  for (const chunk of chunks) {
    const size = chunk.reduce(
      (sum, l) => sum + toBinary(SourcedLogLineSchema, l).length + 6,
      0,
    );
    assert(size <= 64 * 1024, `${size}`);
  }
  assert(64 * 1024 < MAX_CONTROL_MESSAGE_BYTES);
});

test("no lines, no chunks; one line too large still goes alone", () => {
  assertEquals(chunkLines([]), []);
  assertEquals(chunkLines([line(1, 2_000)], 1_000).length, 1);
});

test("a dashboard that reconnects as everything stops is closed", () => {
  // Only what the hub touches before it greets.
  const closes: number[] = [];
  const socket = {
    close: (code: number) => closes.push(code),
  } as unknown as Socket;
  const source = {} as ControlSource;
  const hub = new ControlHub(source, () => {}, () => {});
  hub.close();
  hub.attach(socket);
  socket.onopen!(new Event("open"));
  assertEquals(closes, [1001]);
});
