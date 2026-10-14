/**
 * What the dashboard shows of all nodes at once, gathered once by the dev
 * server from each node's own RPC over IPC (spec 7 §6): its log
 * (`SubscribeLogs`), its peerings (`SubscribeNodes`) and the packets it
 * sends to peers (`SubscribePackets`). The pure parts, a bounded log ring,
 * a node's peer set and a filter that counts each packet once, are tested
 * on their own.
 *
 * @module
 */

import { create } from "@bufbuild/protobuf";
import type { RpcClient } from "@dassie/rpc";
import {
  type Hop,
  HopSchema,
  type SourcedLogLine,
  SourcedLogLineSchema,
} from "@dassie/rpc/dev";
import {
  type LogLine,
  type NetworkNode,
  type NodeTableEvent,
  type Packet,
  PeerState,
} from "@dassie/rpc/proto";
import { subscribe, type Subscription } from "@dassie/ui/live/subscription";

/** Lines the dev server keeps, from all sources. */
export const MAX_LOG_LINES = 10_000;
/** Packets remembered, so each is counted once. */
const MAX_SEEN_PACKETS = 4_096;

/** The last lines of every source, oldest first. */
export class LogRing {
  readonly #lines: SourcedLogLine[] = [];

  constructor(readonly capacity = MAX_LOG_LINES) {}

  add(lines: readonly SourcedLogLine[]): void {
    this.#lines.push(...lines);
    const excess = this.#lines.length - this.capacity;
    if (excess > 0) this.#lines.splice(0, excess);
  }

  get lines(): readonly SourcedLogLine[] {
    return this.#lines;
  }

  get size(): number {
    return this.#lines.length;
  }
}

/** A node's peers, from its node table's events. */
export class PeerSet {
  readonly #states = new Map<string, PeerState>();

  /** Applies an event; whether the peers changed. */
  apply(event: NodeTableEvent): boolean {
    const before = this.peers().join();
    if (event.reset) this.#states.clear();
    for (const node of event.upserted) this.#states.set(node.id, stateOf(node));
    for (const id of event.removed) this.#states.delete(id);
    return this.peers().join() !== before;
  }

  /** The IDs of the nodes it is peered with, sorted. */
  peers(): string[] {
    return [...this.#states]
      .filter(([, state]) => state === PeerState.PEERED)
      .map(([id]) => id)
      .sort();
  }
}

function stateOf(node: NetworkNode): PeerState {
  return node.peerState;
}

/** Counts each packet a node sends to a peer once, whatever its updates. */
export class TrafficFilter {
  readonly #seen = new Set<string>();

  /** The peer the packet went to, if it went to one and is new. */
  hop(packet: Packet): string | undefined {
    if (!packet.destination.startsWith("peer:")) return undefined;
    const key = `${packet.trace?.epoch}:${packet.trace?.sequence}`;
    if (this.#seen.has(key)) return undefined;
    this.#seen.add(key);
    if (this.#seen.size > MAX_SEEN_PACKETS) {
      this.#seen.delete(this.#seen.values().next().value!);
    }
    return packet.destination.slice("peer:".length);
  }
}

/** Where a collector's findings go. */
export interface CollectorSink {
  logs(lines: SourcedLogLine[]): void;
  peers(node: string, peers: string[]): void;
  traffic(hops: Hop[]): void;
  /** A subscription that failed for good. */
  failed(node: string, what: string, message: string): void;
}

/** Follows one running node; started with the node, stopped with it. */
export class Collector {
  readonly #subscriptions: Subscription[] = [];

  constructor(
    readonly node: string,
    readonly client: RpcClient,
    readonly sink: CollectorSink,
    /** The short name of a node ID, if it has one. */
    readonly nameOf: (id: string) => string,
  ) {}

  start(): void {
    this.#subscriptions.push(this.#logs(), this.#peers(), this.#traffic());
  }

  stop(): void {
    for (const subscription of this.#subscriptions) subscription.stop();
    this.#subscriptions.length = 0;
  }

  #onStatus(what: string) {
    return (status: { kind: string; message?: string }) => {
      if (status.kind === "failed") {
        this.sink.failed(this.node, what, status.message ?? "");
      }
    };
  }

  #logs(): Subscription {
    // A new generation sends the ring again: only lines not seen pass.
    let last = 0n;
    return subscribe(this.client, "subscribeLogs", {}, {
      onItem: (event) => {
        const fresh = event.lines.filter((line: LogLine) => line.index > last);
        if (fresh.length === 0) return;
        last = fresh[fresh.length - 1].index;
        this.sink.logs(
          fresh.map((line) =>
            create(SourcedLogLineSchema, { source: this.node, line })
          ),
        );
      },
      onStatus: this.#onStatus("logs"),
    });
  }

  #peers(): Subscription {
    const peers = new PeerSet();
    return subscribe(this.client, "subscribeNodes", {}, {
      onItem: (event) => {
        if (!peers.apply(event)) return;
        this.sink.peers(this.node, peers.peers().map(this.nameOf));
      },
      onStatus: this.#onStatus("peers"),
    });
  }

  #traffic(): Subscription {
    const filter = new TrafficFilter();
    return subscribe(this.client, "subscribePackets", {}, {
      onItem: (event) => {
        if (event.event.case !== "packet") return;
        const to = filter.hop(event.event.value);
        if (to === undefined) return;
        this.sink.traffic([
          create(HopSchema, { from: this.node, to: this.nameOf(to) }),
        ]);
      },
      onStatus: this.#onStatus("traffic"),
    });
  }
}

/** A vanity ID's short name (`d3_…` is `d3`), or the ID itself. */
export function shortName(id: string): string {
  const match = /^(d[1-9]\d{0,8})_/.exec(id);
  return match ? match[1] : id;
}
