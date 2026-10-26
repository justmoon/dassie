/**
 * The STREAM toy's network (spec/areas/website.md#the-simulated-network): a
 * sender and a receiver running the real STREAM state machines of
 * `dassie-stream`, compiled to WASM, and a connector between them, simulated
 * one event at a time on a clock of its own.
 *
 * The connector sits next to the sender. It refuses a packet above the
 * largest amount it forwards with F08, and one beyond the packets it
 * already has in flight with T03, as the original's test network did. It
 * forwards the rest to the receiver, which answers after a round trip
 * drawn from a log-normal distribution: its median is the latency, its
 * spread the jitter. An answer that would come back after the Prepare
 * expires never reaches the sender: the connector rejects the Prepare with
 * R00 when it expires, and a Fulfill that comes later is its loss. The
 * draws come from a seeded generator, so the same settings and seed always
 * take the same course.
 *
 * Nothing here touches the real clock or the page: the toy's view advances
 * the simulation as fast as its time dilation asks, and tests run it to
 * the end at once.
 *
 * @module
 */

import type {
  decodeIlpPacket,
  encodeAmountTooLarge,
  encodeReject,
  StreamConnection,
  StreamEvent,
  StreamServer,
  StreamTransmit,
} from "@dassie/wasm";
import { MAX_SEED, Random } from "./random.ts";

/** What the simulation needs of the STREAM module, in Node or a browser. */
export interface StreamModule {
  StreamConnection: typeof StreamConnection;
  StreamServer: typeof StreamServer;
  decodeIlpPacket: typeof decodeIlpPacket;
  encodeAmountTooLarge: typeof encodeAmountTooLarge;
  encodeReject: typeof encodeReject;
}

/** How a run is set up. */
export interface Settings {
  /** What the sender sends, in the asset's smallest unit. */
  amount: bigint;
  /** The largest packet the connector forwards; larger ones get F08. */
  maxPacketAmount: bigint;
  /** The median round trip from the connector to the receiver, in ms. */
  latency: number;
  /** The spread of the round trip: the σ of its log-normal distribution. */
  jitter: number;
  /** How many packets the connector forwards at once; more get T03. */
  maxPacketsInFlight: number;
  /** Seeds the round trips. */
  seed: number;
}

/** The original's defaults, with a seed. */
export const DEFAULT_SETTINGS: Settings = {
  amount: 3000n,
  maxPacketAmount: 1000n,
  latency: 100,
  jitter: 0.2,
  maxPacketsInFlight: 10,
  seed: 1,
};

const MAX_U64 = 0xffff_ffff_ffff_ffffn;
/** The longest median round trip, in ms. */
export const MAX_LATENCY = 60_000;
/** The widest spread. */
export const MAX_JITTER = 2;
/** The most packets in flight. */
export const MAX_IN_FLIGHT = 1000;
/** A run stops after this many packets… */
export const MAX_PACKETS = 2000;
/** …or after this long on its own clock, in ms. */
export const MAX_DURATION = 10 * 60_000;
/** The most log lines kept. */
export const MAX_LOGS = 10_000;

/** Why `settings` cannot run, or nothing if they can. */
export function checkSettings(settings: Settings): string | undefined {
  const { amount, maxPacketAmount, latency, jitter, maxPacketsInFlight } =
    settings;
  if (amount < 1n || amount > MAX_U64) {
    return "The amount must be between 1 and 2⁶⁴ − 1.";
  }
  if (maxPacketAmount < 1n || maxPacketAmount > MAX_U64) {
    return "The largest packet must be between 1 and 2⁶⁴ − 1.";
  }
  if (!(latency >= 0 && latency <= MAX_LATENCY)) {
    return `The latency must be between 0 and ${MAX_LATENCY} ms.`;
  }
  if (!(jitter >= 0 && jitter <= MAX_JITTER)) {
    return `The jitter must be between 0 and ${MAX_JITTER}.`;
  }
  if (
    !Number.isInteger(maxPacketsInFlight) || maxPacketsInFlight < 1 ||
    maxPacketsInFlight > MAX_IN_FLIGHT
  ) {
    return `Packets in flight must be a whole number from 1 to ${MAX_IN_FLIGHT}.`;
  }
  if (
    !Number.isInteger(settings.seed) || settings.seed < 0 ||
    settings.seed > MAX_SEED
  ) {
    return `The seed must be a whole number from 0 to ${MAX_SEED}.`;
  }
  return undefined;
}

/** The two ends of the connection. */
export type Side = "sender" | "receiver";

/** The ILP addresses of the two ends, as the original named them. */
export const ADDRESSES: Record<Side, string> = {
  sender: "test.client",
  receiver: "test.server",
};

/** The connector's ILP address, which its rejects name. */
export const CONNECTOR = "test.connector";

/** An ILP Fulfill or Reject, as the sender of the Prepare got it. */
export interface Response {
  /** When it arrived, on the simulation's clock. */
  at: number;
  /** The packet. */
  bytes: Uint8Array;
  /** Whether it is a Fulfill. */
  fulfilled: boolean;
  /** A Reject's code, such as "F08". */
  code: string;
  /** A Reject's message. */
  message: string;
  /** A Reject's triggered-by address. */
  triggeredBy: string;
  /** The data, for the receiver's STREAM response or F08's amounts. */
  data: Uint8Array;
}

/** An ILP Prepare one end sent, and its answer once it arrives. */
export interface PacketRecord {
  /** Counts from 0 in the order they were sent. */
  index: number;
  /** Who sent it. */
  from: Side;
  /** When it was sent, on the simulation's clock. */
  sentAt: number;
  /** The packet. */
  bytes: Uint8Array;
  amount: bigint;
  destination: string;
  /** Its expiry, on the simulation's clock. */
  expiresAt: number;
  /** The STREAM request it carries, encrypted. */
  data: Uint8Array;
  /** Its answer, once it has arrived. */
  response?: Response;
}

/** A log line, for the toy's log viewer. */
export interface LogEntry {
  /** On the simulation's clock. */
  at: number;
  level: "debug" | "info" | "warn";
  /** Who logged it: an end or the connector. */
  source: Side | "connector";
  message: string;
  fields: [string, string][];
}

/** How a run stands. */
export type Status =
  | { state: "running" }
  | { state: "done"; delivered: bigint }
  | { state: "stopped"; reason: string };

/** Something due at a time: a packet arriving somewhere. */
interface Delivery {
  at: number;
  /** Breaks ties in the order they were scheduled. */
  order: number;
  kind: "prepare" | "response";
  record: PacketRecord;
  requestId: bigint;
  /** The server's connection that sent the Prepare, if the receiver did. */
  connection: bigint | undefined;
  /** For a response: the packet; for a Prepare: unused. */
  response: Uint8Array | undefined;
  /** Whether the connector forwarded it, and so counts it in flight. */
  forwarded: boolean;
  /**
   * For a response: whether it is the connector's R00 at the Prepare's
   * expiry, which the receiver's later answer does not replace.
   */
  timedOut: boolean;
}

/** A run of the toy. Call {@link dispose} when done with it. */
export class Simulation {
  readonly settings: Settings;
  /** When the run started, on its own clock. */
  readonly start: number;
  /** The connection's shared secret, which decrypts its STREAM packets. */
  readonly sharedSecret: Uint8Array;
  /** The Prepares sent so far. */
  readonly packets: PacketRecord[] = [];
  /** The log lines so far. */
  readonly logs: LogEntry[] = [];
  #now: number;
  #status: Status = { state: "running" };
  readonly #stream: StreamModule;
  readonly #random: Random;
  readonly #client: StreamConnection;
  readonly #server: StreamServer;
  readonly #streamId: bigint;
  readonly #connections: bigint[] = [];
  /** Sorted by time, then by the order they were scheduled. */
  readonly #queue: Delivery[] = [];
  #order = 0;
  #inFlight = 0;
  #closing = false;
  #disposed = false;

  constructor(stream: StreamModule, settings: Settings, start: number) {
    const problem = checkSettings(settings);
    if (problem) throw new RangeError(problem);
    this.settings = settings;
    this.start = start;
    this.#now = start;
    this.#stream = stream;
    this.#random = new Random(settings.seed);

    const secret = crypto.getRandomValues(new Uint8Array(32));
    const server = new stream.StreamServer(
      secret,
      ADDRESSES.receiver,
      "XRP",
      9,
      settings.amount,
    );
    let client: StreamConnection | undefined;
    try {
      const credentials = server.generateCredentials(new Uint8Array());
      this.sharedSecret = credentials.sharedSecret;
      const destination = credentials.destination;
      credentials.free();
      client = new stream.StreamConnection(
        this.sharedSecret,
        destination,
        ADDRESSES.sender,
        "XRP",
        9,
      );
      client.dangerouslyIgnoreExchangeRate();
      this.#streamId = client.openStream();
      client.setSendMax(this.#streamId, settings.amount);
      this.#server = server;
      this.#client = client;
      this.#log("sender", "info", `sending ${settings.amount}`, [
        ["destination", destination],
      ]);
      this.#pump();
    } catch (error) {
      client?.free();
      server.free();
      throw error;
    }
  }

  /** The simulation's clock, in ms since the epoch. */
  get now(): number {
    return this.#now;
  }

  get status(): Status {
    return this.#status;
  }

  /** The packets the connector has forwarded and not yet answered. */
  get inFlight(): number {
    return this.#inFlight;
  }

  /** What the receiver has acknowledged so far. */
  get delivered(): bigint {
    return this.#disposed ? 0n : this.#client.totalSent(this.#streamId);
  }

  /** When the next event is due, or nothing if none is. */
  next(): number | undefined {
    if (this.#status.state !== "running") return undefined;
    // Once the sender has closed and nothing is on the way, the run is
    // over, whatever the receiver still keeps timers for.
    if (this.#client.isClosed() && this.#queue.length === 0) return undefined;
    const times = [
      this.#queue[0]?.at,
      this.#client.pollTimeout(),
      this.#server.pollTimeout(),
    ].filter((time) => time !== undefined);
    return times.length === 0 ? undefined : Math.min(...times);
  }

  /** Runs the next event. Returns false once nothing is left to run. */
  step(): boolean {
    const at = this.next();
    if (at === undefined) {
      this.#finish();
      return false;
    }
    this.#now = Math.max(this.#now, at);
    if (this.#now - this.start > MAX_DURATION) {
      this.#stop(`it ran for more than ${MAX_DURATION / 60_000} minutes`);
      return false;
    }
    const head = this.#queue[0];
    if (head && head.at <= this.#now) {
      this.#queue.shift();
      if (head.kind === "prepare") this.#arrive(head);
      else this.#answer(head);
    } else {
      this.#client.handleTimeout(this.#now);
      this.#server.handleTimeout(this.#now);
    }
    this.#pump();
    if (this.next() === undefined) this.#finish();
    return this.#status.state === "running";
  }

  /**
   * Runs what is due up to `time`, at most `limit` events, and moves the
   * clock there. Returns how many events ran.
   */
  advanceTo(time: number, limit = 1000): number {
    let count = 0;
    for (; count < limit; count++) {
      const at = this.next();
      if (at === undefined || at > time) break;
      this.step();
    }
    if (count < limit && this.#status.state === "running") {
      this.#now = Math.max(this.#now, time);
    }
    return count;
  }

  /** Stops the run because something went wrong outside it. */
  fail(error: unknown): void {
    this.#stop(`an error: ${error instanceof Error ? error.message : error}`);
  }

  /** Frees the STREAM state machines. */
  dispose(): void {
    if (this.#disposed) return;
    this.#disposed = true;
    if (this.#status.state === "running") this.#stop("it was closed");
    this.#client.free();
    this.#server.free();
  }

  /** Sends what the ends have to send and logs what happened to them. */
  #pump(): void {
    // Each round sends or reports something; a connection has a bounded
    // amount of either, so this ends well before the bound.
    for (let round = 0; round < 10_000; round++) {
      if (this.#status.state !== "running") return;
      let progressed = false;
      for (;;) {
        const opened = this.#server.pollOpened();
        if (opened === undefined) break;
        this.#connections.push(opened);
        this.#log("receiver", "info", "connection opened", [
          ["connection", String(opened)],
        ]);
      }
      for (;;) {
        const transmit = this.#client.pollTransmit(this.#now);
        if (!transmit) break;
        this.#send("sender", transmit, undefined);
        progressed = true;
      }
      for (const id of this.#connections) {
        for (;;) {
          const transmit = this.#server.pollTransmit(id, this.#now);
          if (!transmit) break;
          this.#send("receiver", transmit, id);
          progressed = true;
        }
      }
      for (;;) {
        const event = this.#client.pollEvent();
        if (!event) break;
        this.#report("sender", event, undefined);
        progressed = true;
      }
      for (const id of this.#connections) {
        for (;;) {
          const event = this.#server.pollEvent(id);
          if (!event) break;
          this.#report("receiver", event, id);
          progressed = true;
        }
      }
      if (
        !this.#closing &&
        this.#client.totalSent(this.#streamId) >= this.settings.amount
      ) {
        this.#closing = true;
        this.#log("sender", "info", "all sent, closing the connection", []);
        this.#client.close(this.#now, 1);
        progressed = true;
      }
      if (!progressed) return;
    }
    throw new Error("the STREAM state machines did not settle");
  }

  /** The connector takes a Prepare from one end. */
  #send(from: Side, transmit: StreamTransmit, connection: bigint | undefined) {
    const bytes = transmit.packet;
    const requestId = transmit.requestId;
    transmit.free();
    if (this.packets.length >= MAX_PACKETS) {
      this.#stop(`it sent more than ${MAX_PACKETS} packets`);
      return;
    }
    const prepare = this.#stream.decodeIlpPacket(bytes);
    const record: PacketRecord = {
      index: this.packets.length,
      from,
      sentAt: this.#now,
      bytes,
      amount: prepare.amount,
      destination: prepare.address,
      expiresAt: prepare.expiresAt,
      data: prepare.data,
    };
    prepare.free();
    this.packets.push(record);
    const delivery = {
      record,
      requestId,
      connection,
      response: undefined,
      forwarded: false,
      timedOut: false,
    };

    const { maxPacketAmount, maxPacketsInFlight } = this.settings;
    if (this.#inFlight >= maxPacketsInFlight) {
      this.#refuse(delivery, "T03", "too many packets in flight", [
        ["in flight", String(this.#inFlight)],
      ]);
    } else if (record.amount > maxPacketAmount) {
      this.#refuse(
        delivery,
        "F08",
        "amount too large",
        [["amount", String(record.amount)], [
          "maximum",
          String(maxPacketAmount),
        ]],
        this.#stream.encodeAmountTooLarge(record.amount, maxPacketAmount),
      );
    } else {
      this.#inFlight++;
      const trip = Math.round(
        this.#random.logNormal(this.settings.latency, this.settings.jitter),
      );
      this.#log("connector", "debug", "forwarding", [
        ["packet", String(record.index)],
        ["amount", String(record.amount)],
        ["round trip", `${trip} ms`],
      ]);
      // There halfway, back after the whole round trip; the response is
      // computed when the Prepare arrives. An answer that would come back
      // at or after the expiry is replaced by the connector's R00 then.
      const arrives = this.#now + Math.floor(trip / 2);
      const returns = this.#now + trip;
      const timedOut = returns >= record.expiresAt;
      if (arrives < record.expiresAt) {
        this.#schedule({
          ...delivery,
          kind: "prepare",
          at: arrives,
          forwarded: true,
        });
      }
      this.#schedule({
        ...delivery,
        kind: "response",
        at: timedOut ? record.expiresAt : returns,
        forwarded: true,
        timedOut,
        response: timedOut
          ? this.#stream.encodeReject(
            "R00",
            CONNECTOR,
            "expired",
            new Uint8Array(),
          )
          : undefined,
      });
    }
  }

  /** The connector answers a Prepare itself. */
  #refuse(
    delivery: Omit<Delivery, "at" | "order" | "kind">,
    code: string,
    message: string,
    fields: [string, string][],
    data: Uint8Array = new Uint8Array(),
  ): void {
    this.#log("connector", "info", `${code} ${message}`, [
      ["packet", String(delivery.record.index)],
      ...fields,
    ]);
    this.#schedule({
      ...delivery,
      kind: "response",
      at: this.#now,
      response: this.#stream.encodeReject(code, CONNECTOR, message, data),
    });
  }

  /** A forwarded Prepare reaches the other end, which answers it. */
  #arrive(delivery: Delivery): void {
    const { record } = delivery;
    if (this.#now >= record.expiresAt) {
      throw new Error("a Prepare arrived after its expiry");
    }
    const response = record.from === "sender"
      ? this.#server.handlePrepare(this.#now, record.bytes)
      : this.#client.handlePrepare(this.#now, record.bytes);
    const pending = this.#queue.find((other) =>
      other.kind === "response" && other.record === record
    );
    if (!pending) throw new Error("a forwarded Prepare has no response due");
    if (!pending.timedOut) {
      pending.response = response;
      return;
    }
    // The answer comes back too late: the connector has rejected the
    // Prepare by then, and pays for a Fulfill itself.
    const packet = this.#stream.decodeIlpPacket(response);
    const fulfilled = packet.type === 13;
    packet.free();
    if (fulfilled) {
      this.#log("connector", "warn", "a Fulfill will come back too late", [
        ["packet", String(record.index)],
        ["loss", String(record.amount)],
      ]);
    }
  }

  /** An answer reaches the end that sent the Prepare. */
  #answer(delivery: Delivery): void {
    const { record, response } = delivery;
    if (!response) throw new Error("a response is due before its Prepare");
    if (delivery.forwarded) {
      if (this.#inFlight <= 0) throw new Error("no packet is in flight");
      this.#inFlight--;
    }
    if (delivery.timedOut) {
      this.#log("connector", "info", "R00 expired before an answer came back", [
        ["packet", String(record.index)],
      ]);
    }
    const packet = this.#stream.decodeIlpPacket(response);
    record.response = {
      at: this.#now,
      bytes: response,
      fulfilled: packet.type === 13,
      code: packet.code,
      message: packet.message,
      triggeredBy: packet.address,
      data: packet.data,
    };
    packet.free();
    if (record.from === "sender") {
      this.#client.handleResponse(this.#now, delivery.requestId, response);
    } else if (delivery.connection !== undefined) {
      this.#server.handleResponse(
        delivery.connection,
        this.#now,
        delivery.requestId,
        response,
      );
    }
  }

  #schedule(delivery: Omit<Delivery, "order">): void {
    const entry = { ...delivery, order: this.#order++ };
    let index = this.#queue.length;
    while (
      index > 0 && this.#queue[index - 1].at > entry.at
    ) {
      index--;
    }
    this.#queue.splice(index, 0, entry);
  }

  /** Logs an event of one end. */
  #report(side: Side, event: StreamEvent, connection: bigint | undefined) {
    const { kind, streamId, code, byRemote, first, second } = event;
    event.free();
    const stream: [string, string] = ["stream", String(streamId)];
    switch (kind) {
      case "connected":
        return this.#log(side, "info", "connected", []);
      case "streamOpened":
        return this.#log(side, "info", "stream opened", [stream]);
      case "moneySent":
        return this.#log(side, "debug", "money sent", [
          stream,
          ["total", String(this.#client.totalSent(streamId))],
        ]);
      case "moneyReceived": {
        const total = connection === undefined
          ? this.#client.totalReceived(streamId)
          : this.#server.totalReceived(connection, streamId);
        return this.#log(side, "info", "money received", [
          stream,
          ["total", String(total)],
        ]);
      }
      case "streamClosed":
        return this.#log(side, "info", "stream closed", [
          stream,
          ["code", String(code)],
          ["sent", String(first)],
          ["received", String(second)],
        ]);
      case "connectionClosed":
        return this.#log(side, "info", "connection closed", [
          ["code", String(code)],
          ["by", byRemote ? "the other end" : "this end"],
        ]);
      case "moneyStopped":
        return this.#log(side, "warn", "money stopped: packets kept failing", [
          ["expected", String(first)],
          ["received", String(second)],
        ]);
      default:
        return this.#log(side, "debug", kind, []);
    }
  }

  #log(
    source: LogEntry["source"],
    level: LogEntry["level"],
    message: string,
    fields: [string, string][],
  ): void {
    if (this.logs.length >= MAX_LOGS) return;
    this.logs.push({ at: this.#now, level, source, message, fields });
    if (this.logs.length === MAX_LOGS) {
      this.logs.push({
        at: this.#now,
        level: "warn",
        source: "connector",
        message: `log full after ${MAX_LOGS} lines`,
        fields: [],
      });
    }
  }

  #finish(): void {
    if (this.#status.state !== "running") return;
    const delivered = this.delivered;
    this.#status = { state: "done", delivered };
    this.#log("connector", "info", "simulation finished", [
      ["delivered", String(delivered)],
      ["packets", String(this.packets.length)],
    ]);
  }

  #stop(reason: string): void {
    if (this.#status.state !== "running") return;
    this.#status = { state: "stopped", reason };
    this.#log("connector", "warn", `simulation stopped: ${reason}`, []);
  }
}
