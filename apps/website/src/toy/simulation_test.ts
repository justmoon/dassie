// The STREAM toy's network runs the real STREAM state machines (the Deno
// build of the same WASM module the page loads) to the end.

import { test } from "node:test";
import {
  assert,
  assertEquals,
  assertNotEquals,
  assertThrows,
} from "@std/assert";
import * as stream from "@dassie/identity-wasm";
import {
  checkSettings,
  DEFAULT_SETTINGS,
  MAX_PACKETS,
  type PacketRecord,
  type Settings,
  Simulation,
} from "./simulation.ts";
import { Random } from "./random.ts";

const START = Date.UTC(2026, 0, 1);

function run(changes: Partial<Settings> = {}): Simulation {
  const simulation = new Simulation(
    stream,
    { ...DEFAULT_SETTINGS, ...changes },
    START,
  );
  for (let step = 0; step < 100_000 && simulation.step(); step++);
  assert(simulation.status.state !== "running", "the run did not end");
  if (simulation.status.state === "done") checkEnd(simulation);
  return simulation;
}

/** What holds at the end of every run that ran out of events. */
function checkEnd(simulation: Simulation): void {
  assertEquals(simulation.inFlight, 0);
  let fulfilled = 0n;
  for (const packet of simulation.packets) {
    const { response } = packet;
    assert(response, `packet ${packet.index} has no answer`);
    // Nothing comes back after the Prepare expired.
    assert(response.at <= packet.expiresAt, `packet ${packet.index} late`);
    if (packet.from === "sender" && response.fulfilled) {
      fulfilled += packet.amount;
    }
  }
  // What the sender counts as delivered is what came back fulfilled.
  assertEquals(simulation.delivered, fulfilled);
}

/** Runs to the end in frames, as the page does. */
function play(changes: Partial<Settings>, frameMs: number): Simulation {
  const simulation = new Simulation(
    stream,
    { ...DEFAULT_SETTINGS, ...changes },
    START,
  );
  for (
    let frame = 0;
    frame < 100_000 && simulation.status.state === "running";
    frame++
  ) {
    simulation.advanceTo(simulation.now + frameMs);
  }
  assert(simulation.status.state !== "running", "the run did not end");
  checkEnd(simulation);
  return simulation;
}

/** What a run did, without what its random secret changes. */
function course(packets: PacketRecord[]) {
  return packets.map((packet) => ({
    from: packet.from,
    sentAt: packet.sentAt - START,
    amount: packet.amount,
    answeredAt: packet.response && packet.response.at - START,
    code: packet.response?.fulfilled ? "fulfill" : packet.response?.code,
  }));
}

test("the original's defaults deliver the whole amount", () => {
  const simulation = run();
  try {
    assertEquals(simulation.status, { state: "done", delivered: 3000n });
    const fulfilled = simulation.packets.filter((packet) =>
      packet.response?.fulfilled
    );
    const total = fulfilled
      .filter((packet) => packet.from === "sender")
      .reduce((sum, packet) => sum + packet.amount, 0n);
    assertEquals(total, 3000n);
    // Packets above the connector's limit were refused with F08, and none
    // that it forwarded was larger.
    for (const packet of simulation.packets) {
      if (packet.amount > 1000n) {
        assertEquals(packet.response?.code, "F08", `packet ${packet.index}`);
      }
    }
    assert(simulation.packets.every((packet) => packet.response));
  } finally {
    simulation.dispose();
  }
});

test("the same seed takes the same course", () => {
  const first = run({ seed: 7, jitter: 1 });
  const second = run({ seed: 7, jitter: 1 });
  const other = run({ seed: 8, jitter: 1 });
  try {
    assertEquals(course(first.packets), course(second.packets));
    assertNotEquals(course(first.packets), course(other.packets));
  } finally {
    first.dispose();
    second.dispose();
    other.dispose();
  }
});

test("playing in frames takes the same course as stepping", () => {
  const changes = { seed: 3, jitter: 1, amount: 20_000n };
  const stepped = run(changes);
  const played = play(changes, 16.7);
  try {
    assertEquals(course(played.packets), course(stepped.packets));
  } finally {
    stepped.dispose();
    played.dispose();
  }
});

test("an answer later than the expiry becomes the connector's R00", () => {
  // Every round trip takes 35 s, longer than a Prepare's 30 s: the
  // receiver gets each one in time, but no answer gets back in time.
  const simulation = run({ latency: 35_000, jitter: 0 });
  try {
    assertEquals(simulation.delivered, 0n);
    const forwarded = simulation.packets.filter((packet) =>
      packet.amount <= 1000n
    );
    assert(forwarded.length > 0);
    for (const packet of forwarded) {
      assertEquals(packet.response?.code, "R00", `packet ${packet.index}`);
      assertEquals(packet.response?.at, packet.expiresAt);
    }
    assert(
      simulation.logs.some((line) =>
        line.message === "a Fulfill will come back too late"
      ),
    );
  } finally {
    simulation.dispose();
  }
});

test("round trips around the expiry are answered in time or by R00", () => {
  for (const seed of [1, 2, 3, 4, 5]) {
    const simulation = run({ latency: 15_000, jitter: 1, seed });
    simulation.dispose();
  }
});

test("a busy connector refuses with T03, and the sender still delivers", () => {
  const simulation = run({
    amount: 50_000n,
    maxPacketAmount: 1000n,
    maxPacketsInFlight: 2,
  });
  try {
    assertEquals(simulation.status, { state: "done", delivered: 50_000n });
    assert(
      simulation.packets.some((packet) => packet.response?.code === "T03"),
      "no T03",
    );
  } finally {
    simulation.dispose();
  }
});

test("no latency delivers too", () => {
  const simulation = run({ latency: 0, jitter: 0 });
  try {
    assertEquals(simulation.status, { state: "done", delivered: 3000n });
    assertEquals(simulation.now, START);
  } finally {
    simulation.dispose();
  }
});

test("a run stops after its packet limit", () => {
  const simulation = run({ amount: 1_000_000n, maxPacketAmount: 1n });
  try {
    assertEquals(simulation.status.state, "stopped");
    assertEquals(simulation.packets.length, MAX_PACKETS);
  } finally {
    simulation.dispose();
  }
});

test("the packets decode with the connection's shared secret", () => {
  const simulation = run();
  try {
    const { sharedSecret, packets } = simulation;
    const first = packets[0];
    const request = stream.decodeStreamPacket(sharedSecret, first.data);
    assertEquals(request.type, 12);
    const frames = request.frames;
    const kinds = frames.map((frame) => frame.kind);
    assert(kinds.includes("streamMoney"), kinds.join());
    for (const frame of frames) frame.free();
    request.free();

    const tooLarge = packets.find((packet) => packet.response?.code === "F08");
    assert(tooLarge?.response, "no F08");
    assertEquals(
      [...stream.decodeAmountTooLarge(tooLarge.response.data) ?? []],
      [tooLarge.amount, 1000n],
    );

    const fulfilled = packets.find((packet) =>
      packet.from === "sender" && packet.response?.fulfilled
    );
    assert(fulfilled?.response, "no Fulfill");
    const response = stream.decodeStreamPacket(
      sharedSecret,
      fulfilled.response.data,
    );
    assertEquals(response.type, 13);
    assertEquals(response.prepareAmount, fulfilled.amount);
    response.free();

    assertThrows(() =>
      stream.decodeStreamPacket(new Uint8Array(32), first.data)
    );
  } finally {
    simulation.dispose();
  }
});

test("settings out of range are refused", () => {
  const bad: Partial<Settings>[] = [
    { amount: 0n },
    { amount: 1n << 64n },
    { maxPacketAmount: 0n },
    { latency: -1 },
    { latency: Number.NaN },
    { jitter: 3 },
    { maxPacketsInFlight: 0 },
    { maxPacketsInFlight: 1.5 },
    { seed: -1 },
    { seed: 2 ** 32 },
  ];
  for (const changes of bad) {
    const settings = { ...DEFAULT_SETTINGS, ...changes };
    assert(checkSettings(settings), JSON.stringify(changes, String));
    assertThrows(() => new Simulation(stream, settings, START), RangeError);
  }
  assertEquals(checkSettings(DEFAULT_SETTINGS), undefined);
});

test("the generator is seeded and log-normal around its median", () => {
  const a = new Random(42);
  const b = new Random(42);
  for (let i = 0; i < 100; i++) assertEquals(a.next(), b.next());
  const random = new Random(1);
  const samples = Array.from(
    { length: 10_001 },
    () => random.logNormal(100, 0.5),
  )
    .sort((x, y) => x - y);
  const median = samples[5000];
  assert(median > 95 && median < 105, `median ${median}`);
  assert(samples.every((sample) => sample > 0));
  assertEquals(new Random(1).logNormal(0, 1), 0);
});
