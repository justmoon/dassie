/** The dev nodes as the dashboard names and links them. */

import { type Node, NodeState } from "@dassie/rpc/dev";

const STATES: Record<NodeState, string> = {
  [NodeState.UNSPECIFIED]: "unknown",
  [NodeState.STARTING]: "starting",
  [NodeState.SETTING_UP]: "setting up",
  [NodeState.RUNNING]: "running",
  [NodeState.STOPPED]: "stopped",
  [NodeState.FAILED]: "failed",
};

/** A node's state, in a word. */
export function stateName(state: NodeState): string {
  return STATES[state] ?? "unknown";
}

/**
 * The link that opens a node's wallet, signed in
 * (spec/areas/dev.md#wallet-links).
 */
export function walletLink(node: Node): string {
  return `${node.url}/#dassie-dev-login=${node.loginProofHex}`;
}

/**
 * The page of node `name`: the same debug tab as the node shown now, if
 * one is, so that switching nodes keeps the tab.
 */
export function nodePath(name: string, pathname: string): string {
  const tab = /^\/nodes\/d\d+(\/debug\/[a-z]+)/.exec(pathname)?.[1];
  return `/nodes/${name}${tab ?? ""}`;
}
