import { createSignal } from "@dassie/lib-reactive"

import { NodeId } from "../types/node-id"

interface NodeListEntry {
  entries: NodeId[]
  hash: Uint8Array
}

export const BootstrapNodeListsSignal = () =>
  createSignal(new Map<NodeId, NodeListEntry>())
