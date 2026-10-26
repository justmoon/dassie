/**
 * An asynchronous driver for the sans-IO STREAM state machines: it sends
 * the packets a connection produces through an {@link IlpEndpoint}, feeds
 * the responses back, and runs the connection's timers. The interop tests
 * drive the WASM build's connections with it against
 * `ilp-protocol-stream`; the website's toy steps its connections itself.
 *
 * @module
 */

import type { StreamConnection, StreamEvent, StreamServer } from "@dassie/wasm";

/** Something that carries ILP packets: a plugin, a socket, a simulator. */
export interface IlpEndpoint {
  /** Sends an ILP Prepare and resolves with the ILP Fulfill or Reject. */
  sendPacket(prepare: Uint8Array): Promise<Uint8Array>;
}

/** Receives a connection's events. */
export type EventListener = (event: StreamEvent) => void;

/** Options of {@link ClientDriver.pay}. */
export interface PayOptions {
  /** The minimum rate; measured if absent. */
  exchangeRate?: readonly [bigint, bigint];
  /** Give up after this long. */
  timeoutMs?: number;
}

/** The outcome of {@link ClientDriver.pay}. */
export interface PayResult {
  /** The stream the payment used. */
  streamId: bigint;
  /** What the stream sent and the receiver acknowledged. */
  sent: bigint;
}

/** Drives a client {@link StreamConnection}. */
export class ClientDriver {
  readonly connection: StreamConnection;
  readonly #endpoint: IlpEndpoint;
  readonly #listeners = new Set<EventListener>();
  #timer: ReturnType<typeof setTimeout> | undefined;
  #rateKnown = false;

  constructor(connection: StreamConnection, endpoint: IlpEndpoint) {
    this.connection = connection;
    this.#endpoint = endpoint;
  }

  /** Adds an event listener; returns a function that removes it. */
  on(listener: EventListener): () => void {
    this.#listeners.add(listener);
    return () => this.#listeners.delete(listener);
  }

  /** Answers a Prepare that the network delivers to this client. */
  handlePrepare(prepare: Uint8Array): Uint8Array {
    const response = this.connection.handlePrepare(Date.now(), prepare);
    this.pump();
    return response;
  }

  /** Sends whatever is due, delivers events and arms the timer. */
  pump(): void {
    for (;;) {
      const transmit = this.connection.pollTransmit(Date.now());
      if (!transmit) break;
      const requestId = transmit.requestId;
      this.#endpoint.sendPacket(transmit.packet).then(
        (response) => {
          this.connection.handleResponse(Date.now(), requestId, response);
          this.pump();
        },
        // A failed send counts as lost: the packet's expiry settles it.
        () => {},
      );
    }
    for (;;) {
      const event = this.connection.pollEvent();
      if (!event) break;
      if (event.kind === "exchangeRateMeasured") this.#rateKnown = true;
      for (const listener of this.#listeners) listener(event);
    }
    this.#arm();
  }

  #arm(): void {
    if (this.#timer !== undefined) clearTimeout(this.#timer);
    this.#timer = undefined;
    const deadline = this.connection.pollTimeout();
    if (deadline === undefined) return;
    this.#timer = setTimeout(() => {
      this.#timer = undefined;
      this.connection.handleTimeout(Date.now());
      this.pump();
    }, Math.max(0, deadline - Date.now()));
  }

  /** Waits for an event matching `predicate`, or fails after `timeoutMs`. */
  waitFor(
    predicate: (event: StreamEvent) => boolean,
    timeoutMs = 30_000,
  ): Promise<StreamEvent> {
    return new Promise((resolve, reject) => {
      const timer = setTimeout(() => {
        remove();
        reject(new Error("timed out waiting for a STREAM event"));
      }, timeoutMs);
      const remove = this.on((event) => {
        if (!predicate(event)) return;
        clearTimeout(timer);
        remove();
        resolve(event);
      });
      this.pump();
    });
  }

  /** Measures the exchange rate and adopts it, less slippage. */
  async measureExchangeRate(): Promise<readonly [bigint, bigint]> {
    this.connection.measureExchangeRate();
    const event = await this.waitFor((event) =>
      event.kind === "exchangeRateMeasured" ||
      event.kind === "exchangeRateMeasurementFailed" ||
      event.kind === "connectionClosed"
    );
    if (event.kind !== "exchangeRateMeasured") {
      throw new Error(`exchange rate measurement failed: ${event.kind}`);
    }
    return [event.first, event.second];
  }

  /** Sends `amount` on a new stream and resolves once it is delivered. */
  async pay(amount: bigint, options: PayOptions = {}): Promise<PayResult> {
    if (options.exchangeRate) {
      this.connection.setExchangeRate(...options.exchangeRate);
      this.#rateKnown = true;
    } else if (!this.#rateKnown) {
      await this.measureExchangeRate();
    }
    const streamId = this.connection.openStream();
    this.connection.setSendMax(streamId, amount);
    const done = () => this.connection.totalSent(streamId) >= amount;
    if (!done()) {
      const event = await this.waitFor(
        (event) =>
          done() || event.kind === "connectionClosed" ||
          event.kind === "moneyStopped",
        options.timeoutMs,
      );
      if (!done()) throw new Error(`payment stopped: ${event.kind}`);
    }
    return { streamId, sent: this.connection.totalSent(streamId) };
  }

  /** Closes the connection and waits until it is closed. */
  async close(): Promise<void> {
    this.connection.close(Date.now(), 1);
    if (!this.connection.isClosed()) {
      await this.waitFor((event) => event.kind === "connectionClosed");
    }
    this.stop();
  }

  /** Stops the timer, so that nothing keeps the process alive. */
  stop(): void {
    if (this.#timer !== undefined) clearTimeout(this.#timer);
    this.#timer = undefined;
  }
}

/** Receives a server's events, with the connection they belong to. */
export type ServerEventListener = (
  connectionId: bigint,
  event: StreamEvent,
) => void;

/** Drives a {@link StreamServer} that only receives. */
export class ServerDriver {
  readonly server: StreamServer;
  readonly #listeners = new Set<ServerEventListener>();
  readonly #connections: bigint[] = [];
  #timer: ReturnType<typeof setTimeout> | undefined;

  constructor(server: StreamServer) {
    this.server = server;
  }

  /** Adds an event listener; returns a function that removes it. */
  on(listener: ServerEventListener): () => void {
    this.#listeners.add(listener);
    return () => this.#listeners.delete(listener);
  }

  /** Answers a Prepare that the network delivers to this server. */
  handlePrepare(prepare: Uint8Array): Uint8Array {
    const response = this.server.handlePrepare(Date.now(), prepare);
    this.pump();
    return response;
  }

  /** Delivers events and arms the timer. */
  pump(): void {
    for (;;) {
      const opened = this.server.pollOpened();
      if (opened === undefined) break;
      this.#connections.push(opened);
    }
    for (const id of this.#connections) {
      for (;;) {
        const event = this.server.pollEvent(id);
        if (!event) break;
        for (const listener of this.#listeners) listener(id, event);
      }
    }
    if (this.#timer !== undefined) clearTimeout(this.#timer);
    this.#timer = undefined;
    const deadline = this.server.pollTimeout();
    if (deadline === undefined) return;
    this.#timer = setTimeout(() => {
      this.#timer = undefined;
      this.server.handleTimeout(Date.now());
      this.pump();
    }, Math.max(0, deadline - Date.now()));
  }

  /** Stops the timer, so that nothing keeps the process alive. */
  stop(): void {
    if (this.#timer !== undefined) clearTimeout(this.#timer);
    this.#timer = undefined;
  }
}
