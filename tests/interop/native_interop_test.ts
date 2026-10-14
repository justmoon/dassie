// The native dassie-stream against ilp-protocol-stream, in both directions,
// through the `interop_peer` example (crates/dassie-stream/examples), which
// speaks length-prefixed frames over stdio.

import { Buffer } from "node:buffer";

import { assert, assertEquals } from "@std/assert";
import { decodeHex, encodeHex } from "@std/encoding/hex";
import {
  type Connection,
  createConnection,
  createServer,
  type DataAndMoneyStream,
  verifyReceipt,
} from "ilp-protocol-stream";

import { encodeReject } from "@dassie/identity-wasm";

import { createPlugin, Network } from "./network.ts";

const options = { sanitizeOps: false, sanitizeResources: false };
const root = new URL("../../", import.meta.url).pathname;

// The Nix check builds the peer itself and names it in DASSIE_INTEROP_PEER,
// as its sandbox has no Cargo registry to build from.
async function buildPeer(): Promise<string> {
  const built = Deno.env.get("DASSIE_INTEROP_PEER");
  if (built) return built;
  const build = await new Deno.Command("cargo", {
    args: ["build", "--package", "dassie-stream", "--example", "interop_peer"],
    cwd: root,
    stdout: "inherit",
    stderr: "inherit",
  }).output();
  assert(build.success, "building the interop peer failed");
  return `${root}target/debug/examples/interop_peer`;
}

/** A running peer and the frames it exchanges. */
class Peer {
  readonly #process: Deno.ChildProcess;
  readonly #writer: WritableStreamDefaultWriter<Uint8Array>;
  readonly #pending = new Map<bigint, (bytes: Uint8Array) => void>();
  readonly lines: string[] = [];
  #nextId = 0n;
  #stopped = false;
  onSend: (requestId: bigint, prepare: Uint8Array) => void = () => {};
  readonly finished: Promise<void>;

  constructor(binary: string, args: string[]) {
    this.#process = new Deno.Command(binary, {
      args,
      stdin: "piped",
      stdout: "piped",
      stderr: "inherit",
    }).spawn();
    this.#writer = this.#process.stdin.getWriter();
    this.finished = this.#read();
  }

  async #read() {
    let buffer = new Uint8Array();
    for await (const chunk of this.#process.stdout) {
      buffer = Uint8Array.from([...buffer, ...chunk]);
      while (buffer.length >= 13) {
        const view = new DataView(buffer.buffer, buffer.byteOffset);
        const length = view.getUint32(9);
        if (buffer.length < 13 + length) break;
        const kind = buffer[0];
        const id = view.getBigUint64(1);
        const bytes = buffer.slice(13, 13 + length);
        buffer = buffer.slice(13 + length);
        this.#handle(kind, id, bytes);
      }
    }
  }

  #handle(kind: number, id: bigint, bytes: Uint8Array) {
    if (kind === 2) {
      this.#pending.get(id)?.(bytes);
      this.#pending.delete(id);
    } else if (kind === 4) {
      this.onSend(id, bytes);
    } else if (kind === 5) {
      this.lines.push(new TextDecoder().decode(bytes));
    }
  }

  #write(kind: number, id: bigint, bytes: Uint8Array) {
    const frame = new Uint8Array(13 + bytes.length);
    const view = new DataView(frame.buffer);
    frame[0] = kind;
    view.setBigUint64(1, id);
    view.setUint32(9, bytes.length);
    frame.set(bytes, 13);
    // The client's send loop may still deliver a packet after the test
    // stopped the peer; that write fails, and nothing waits for it.
    return this.#writer.write(frame).catch(() => {});
  }

  /** Delivers a Prepare to the peer and resolves with its answer. */
  prepare(bytes: Uint8Array): Promise<Uint8Array> {
    // The other side may still send, closing its connection, after the
    // peer stopped: it gets an answer, not a promise that never settles.
    if (this.#stopped) return Promise.resolve(gone());
    const id = this.#nextId++;
    const answer = new Promise<Uint8Array>((resolve) =>
      this.#pending.set(id, resolve)
    );
    void this.#write(1, id, bytes);
    return answer;
  }

  /** Delivers the response to a Prepare the peer sent. */
  reply(requestId: bigint, bytes: Uint8Array) {
    return this.#write(3, requestId, bytes);
  }

  waitForLine(predicate: (line: string) => boolean, timeoutMs = 20_000) {
    return new Promise<string>((resolve, reject) => {
      const started = Date.now();
      const check = () => {
        const line = this.lines.find(predicate);
        if (line) return resolve(line);
        if (Date.now() - started > timeoutMs) {
          return reject(
            new Error(`no matching line in:\n${this.lines.join("\n")}`),
          );
        }
        setTimeout(check, 5);
      };
      check();
    });
  }

  async stop() {
    this.#stopped = true;
    await this.#writer.close().catch(() => {});
    await this.#process.status;
    await this.finished;
    for (const answer of this.#pending.values()) answer(gone());
    this.#pending.clear();
  }
}

/** What a stopped peer answers. */
function gone(): Uint8Array {
  return encodeReject(
    "T01",
    "test.native",
    "the peer stopped",
    new Uint8Array(),
  );
}

Deno.test(
  "native dassie client pays an ilp-protocol-stream server",
  options,
  async () => {
    const binary = await buildPeer();
    const network = new Network();
    const server = await createServer({
      plugin: createPlugin(network, "test.upstream"),
      serverSecret: Buffer.alloc(32, 9),
    });
    let received = 0n;
    server.on("connection", (connection: Connection) => {
      connection.on("stream", (stream: DataAndMoneyStream) => {
        stream.setReceiveMax(10_000_000);
        stream.on("money", (amount: string) => received += BigInt(amount));
      });
    });
    const { destinationAccount, sharedSecret } = server
      .generateAddressAndSecret();

    const peer = new Peer(binary, [
      "client",
      destinationAccount,
      encodeHex(new Uint8Array(sharedSecret)),
      "1000000",
      "test.native",
    ]);
    network.register("test.native", (prepare) => peer.prepare(prepare));
    peer.onSend = (requestId, prepare) => {
      network.route(prepare).then((response) =>
        peer.reply(requestId, response)
      );
    };

    await peer.waitForLine((line) => line === "done");
    assert(peer.lines.includes("rate 1 1"), peer.lines.join("\n"));
    assertEquals(received, 1_000_000n);
    await peer.stop();
    await server.close();
  },
);

Deno.test(
  "ilp-protocol-stream client pays a native dassie server",
  options,
  async () => {
    const binary = await buildPeer();
    const network = new Network();
    const receiptSecret = new Uint8Array(32).fill(4);
    const peer = new Peer(binary, [
      "server",
      "test.native",
      encodeHex(new Uint8Array(16).fill(5)),
      encodeHex(receiptSecret),
    ]);
    network.register("test.native", (prepare) => peer.prepare(prepare));
    peer.onSend = (requestId, prepare) => {
      network.route(prepare).then((response) =>
        peer.reply(requestId, response)
      );
    };
    const line = await peer.waitForLine((line) =>
      line.startsWith("credentials ")
    );
    const [, destination, secret] = line.split(" ");

    const connection = await createConnection({
      plugin: createPlugin(network, "test.upstream"),
      destinationAccount: destination,
      sharedSecret: Buffer.from(decodeHex(secret)),
    });
    const stream = connection.createStream();
    await stream.sendTotal(4321);
    await peer.waitForLine((line) => line === "received 0 1 4321");
    const receipt = verifyReceipt(stream.receipt!, Buffer.from(receiptSecret));
    assertEquals(receipt.totalReceived.toString(), "4321");

    await connection.end();
    await peer.waitForLine((line) => line.startsWith("closed 0 1 true"));
    await peer.stop();
  },
);
