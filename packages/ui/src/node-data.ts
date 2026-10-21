/**
 * What the whole wallet follows while the owner is signed in: the ledgers,
 * with their balances, the exchange rates and the owner's total, the
 * external assets they settle through, and the node table. The gate, the navigation and most pages read them; the dev
 * dashboard keeps one per node.
 */

import type {
  DisplayTotal,
  ExchangeRate,
  ExternalAsset,
  Ledger,
  NetworkNode,
} from "@dassie/rpc/proto";
import {
  type Accessor,
  type Context,
  createContext,
  createSignal,
  type Store,
  useContext,
} from "solid-js";
import { createLiveTable, createLiveValue } from "./live/live.ts";
import type { Keyed } from "./live/reduce.ts";
import type { LiveStatus } from "./live/subscription.ts";

/** The node's live state. */
export interface NodeData {
  /** The ledgers; undefined until the first item. */
  ledgers: Accessor<readonly Ledger[] | undefined>;
  ledgersStatus: Accessor<LiveStatus>;
  /** Takes a ledger the node just reported, before the stream does. */
  applyLedger(ledger: Ledger): void;
  /** The exchange rates the node converts at. */
  rates: Accessor<readonly ExchangeRate[]>;
  /** The currency the owner sees totals in; empty for none. */
  displayCurrency: Accessor<string>;
  /** What the owner has on every ledger, in that currency, while the
   * rates allow. */
  total: Accessor<DisplayTotal | undefined>;
  /** What the ledgers settle through, each with what it holds as its
   * ledger says, never as the books add it up. */
  externalAssets: Accessor<readonly ExternalAsset[]>;
  nodes: Store<Keyed<NetworkNode>>;
  nodesStatus: Accessor<LiveStatus>;
  /** This node's ID. */
  selfId: string;
  /** "test" or "g". */
  allocationScheme: string;
  /** The node's hostname; empty until configured. */
  hostname: string;
}

export const NodeDataContext: Context<NodeData> = createContext<NodeData>();

/** The node's live state. */
export function useNodeData(): NodeData {
  return useContext(NodeDataContext);
}

/** Subscribes to the node's ledgers and node table for the owner's scope. */
export function createNodeData(basic: {
  nodeId: string;
  ilpAllocationScheme: string;
  hostname: string;
}): NodeData {
  // A ledger just added shows at once, before the stream has it, and for
  // a few seconds at most: by then the stream says whether it is there.
  const [added, setAdded] = createSignal<readonly Ledger[]>([]);
  const live = createLiveValue(
    "subscribeLedgers",
    () => ({}),
    (item) => item,
  );
  const ledgers = () => {
    const streamed = live.value()?.ledgers;
    const extra = added().filter((ledger) =>
      !streamed?.some((known) => known.id === ledger.id)
    );
    if (streamed === undefined && extra.length === 0) return undefined;
    return [...(streamed ?? []), ...extra];
  };
  const table = createLiveTable(
    "subscribeNodes",
    () => ({}),
    (item) => item,
    (node: NetworkNode) => node.id,
  );
  return {
    ledgers,
    ledgersStatus: live.status,
    applyLedger: (ledger) => {
      setAdded((all) => [...all, ledger]);
      setTimeout(
        () => setAdded((all) => all.filter((l) => l !== ledger)),
        5_000,
      );
    },
    rates: () => live.value()?.rates ?? [],
    displayCurrency: () => live.value()?.displayCurrency ?? "",
    total: () => live.value()?.total,
    externalAssets: () => live.value()?.externalAssets ?? [],
    nodes: table.state,
    nodesStatus: table.status,
    selfId: basic.nodeId,
    allocationScheme: basic.ilpAllocationScheme,
    hostname: basic.hostname,
  };
}
