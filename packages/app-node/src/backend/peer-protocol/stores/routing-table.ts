import { createSignal } from "@dassie/lib-reactive"

import { NodeId } from "../types/node-id"

export interface RoutingTableEntry {
  distance: number
  firstHopOptions: NodeId[]
}

export const routingTableStore = () =>
  createSignal(new Map<string, RoutingTableEntry>())
