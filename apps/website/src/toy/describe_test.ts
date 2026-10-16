import { test } from "node:test";
import { assert, assertEquals } from "@std/assert";
import * as stream from "@dassie/identity-wasm";
import {
  decodeStream,
  decodeTooLarge,
  describeFrame,
  type Frame,
  frameName,
  ilpErrorName,
  streamErrorName,
} from "./describe.ts";
import { DEFAULT_SETTINGS, Simulation } from "./simulation.ts";

const frame = (changes: Partial<Frame>): Frame => ({
  kind: "",
  streamId: 1n,
  code: 0,
  text: "",
  first: 0n,
  second: 0n,
  length: 0,
  ...changes,
});

test("error codes have names", () => {
  assertEquals(ilpErrorName("F08"), "Amount Too Large");
  assertEquals(ilpErrorName("T03"), "Connector Busy");
  assertEquals(ilpErrorName("X42"), "Unknown Error");
  assertEquals(streamErrorName(1), "NoError");
  assertEquals(streamErrorName(9), "ApplicationError");
  assertEquals(streamErrorName(255), "Unknown (0xff)");
  assertEquals(streamErrorName(0), "Unknown (0x00)");
});

test("frames read as words", () => {
  const cases: [Partial<Frame>, string, string][] = [
    [
      { kind: "streamMoney", first: 1000n },
      "StreamMoney",
      "stream 1: 1000 shares",
    ],
    [
      { kind: "connectionClose", code: 1, text: "" },
      "ConnectionClose",
      "NoError",
    ],
    [
      { kind: "streamClose", code: 9, text: "bye" },
      "StreamClose",
      "stream 1, ApplicationError: bye",
    ],
    [
      { kind: "connectionAssetDetails", text: "XRP", first: 9n },
      "ConnectionAssetDetails",
      "XRP, scale 9",
    ],
    [
      { kind: "streamMaxMoney", first: 3000n, second: 1000n },
      "StreamMaxMoney",
      "stream 1: receives up to 3000, has 1000",
    ],
    [
      { kind: "connectionNewAddress", text: "" },
      "ConnectionNewAddress",
      "(no address)",
    ],
  ];
  for (const [changes, name, words] of cases) {
    assertEquals(frameName(frame(changes)), name);
    assertEquals(describeFrame(frame(changes)), words);
  }
});

test("a run's packets decode, and foreign data does not", () => {
  const simulation = new Simulation(stream, DEFAULT_SETTINGS, 0);
  try {
    while (simulation.step());
    const [first] = simulation.packets;
    const decoded = decodeStream(stream, simulation.sharedSecret, first.data);
    assert(typeof decoded !== "string", String(decoded));
    assertEquals(decoded.type, 12);
    assertEquals(decoded.prepareAmount, 0n);
    assert(decoded.frames.some((frame) => frame.kind === "streamMoney"));

    assertEquals(
      decodeStream(stream, simulation.sharedSecret, new Uint8Array()),
      "No data",
    );
    assertEquals(
      decodeStream(stream, new Uint8Array(32), first.data),
      "Not a STREAM packet of this connection",
    );
    assertEquals(
      decodeTooLarge(stream, stream.encodeAmountTooLarge(3000n, 1000n)),
      { received: 3000n, maximum: 1000n },
    );
    assertEquals(
      decodeTooLarge(stream, new Uint8Array()),
      "No amounts in the data",
    );
  } finally {
    simulation.dispose();
  }
});
