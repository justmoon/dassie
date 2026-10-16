import { test } from "node:test";
import { assertEquals, assertThrows } from "@std/assert";
import { create } from "@bufbuild/protobuf";
import {
  ClientMessageSchema,
  decodeClientMessage,
  decodeServerMessage,
  encodeClientMessage,
  encodeServerMessage,
  MAX_CONTROL_MESSAGE_BYTES,
  ServerMessageSchema,
} from "./dev.ts";

test("control messages round-trip", () => {
  const server = create(ServerMessageSchema, {
    message: {
      case: "traffic",
      value: { hops: [{ from: "d1", to: "d2" }] },
    },
  });
  assertEquals(decodeServerMessage(encodeServerMessage(server)), server);
  const client = create(ClientMessageSchema, {
    message: { case: "setScenario", value: { id: "six-nodes" } },
  });
  assertEquals(decodeClientMessage(encodeClientMessage(client)), client);
});

test("control messages are bounded both ways", () => {
  const huge = create(ServerMessageSchema, {
    message: {
      case: "traffic",
      value: {
        hops: Array.from({ length: 1 }, () => ({
          from: "x".repeat(MAX_CONTROL_MESSAGE_BYTES),
          to: "d2",
        })),
      },
    },
  });
  assertThrows(() => encodeServerMessage(huge), RangeError);
  assertThrows(
    () => decodeClientMessage(new Uint8Array(MAX_CONTROL_MESSAGE_BYTES + 1)),
    RangeError,
  );
});
