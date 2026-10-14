/**
 * What the dashboard shows, as Solid signals fed by the control channel:
 * the dev server's state, the logs of every source, the peerings, and the
 * links with traffic just now.
 */

import { create } from "@bufbuild/protobuf";
import type { LogLine } from "@dassie/rpc/proto";
import type { ServerMessage, State } from "@dassie/rpc/dev";
import { StateSchema } from "@dassie/rpc/dev";
import {
  type Accessor,
  createContext,
  createSignal,
  onCleanup,
  useContext,
} from "solid-js";
import { type Command, type ConnectionStatus, DevClient } from "./client.ts";
import { Pulses } from "./graph.ts";

/** Lines kept, as many as the dev server keeps. */
const MAX_LINES = 10_000;
/** How often lit links are checked for going dark. */
const PULSE_TICK_MS = 150;

export interface DevStore {
  status: Accessor<ConnectionStatus>;
  state: Accessor<State>;
  lines: Accessor<readonly LogLine[]>;
  /** The source of a line: a node's name, or "host". */
  sourceOf: (line: LogLine) => string;
  peers: Accessor<Readonly<Record<string, readonly string[]>>>;
  lit: Accessor<ReadonlySet<string>>;
  send: (command: Command) => boolean;
}

const DevContext = createContext<DevStore>();
export const DevStoreProvider = DevContext;

export function useDev(): DevStore {
  return useContext(DevContext);
}

/** Connects to the dev server for the owner's lifetime. */
export function createDevStore(url: string): DevStore {
  const [status, setStatus] = createSignal<ConnectionStatus>("connecting");
  const [state, setState] = createSignal<State>(create(StateSchema, {}));
  const [lines, setLines] = createSignal<readonly LogLine[]>([]);
  const [peers, setPeers] = createSignal<Record<string, readonly string[]>>(
    {},
  );
  const pulses = new Pulses();
  const [lit, setLit] = createSignal<ReadonlySet<string>>(new Set());
  // A line's source, without copying every line into a wrapper.
  let sources = new WeakMap<LogLine, string>();

  const onMessage = (message: ServerMessage) => {
    const { case: kind, value } = message.message;
    if (kind === "state") setState(value);
    else if (kind === "logs") {
      if (value.reset) sources = new WeakMap();
      const added: LogLine[] = [];
      for (const { source, line } of value.lines) {
        if (!line) continue;
        sources.set(line, source);
        added.push(line);
      }
      setLines((before) => {
        const all = value.reset ? added : [...before, ...added];
        return all.length > MAX_LINES ? all.slice(-MAX_LINES) : all;
      });
    } else if (kind === "peerings") {
      const next: Record<string, readonly string[]> = {};
      for (const [name, list] of Object.entries(value.peers)) {
        next[name] = list.peers;
      }
      setPeers(next);
    } else if (kind === "traffic") {
      const now = Date.now();
      for (const hop of value.hops) pulses.hit(hop.from, hop.to, now);
      setLit(pulses.lit());
    }
  };
  const client = new DevClient({
    url,
    onMessage,
    onStatus: setStatus,
  });
  client.start();
  const ticker = setInterval(() => {
    if (pulses.expire(Date.now())) setLit(pulses.lit());
  }, PULSE_TICK_MS);
  onCleanup(() => {
    clearInterval(ticker);
    client.close();
  });
  return {
    status,
    state,
    lines,
    sourceOf: (line) => sources.get(line) ?? "",
    peers,
    lit,
    send: (command) => client.send(command),
  };
}
