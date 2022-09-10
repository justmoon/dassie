import { createSignal } from "@dassie/lib-reactive"

export interface RoutingTableEntry {
  distance: number
  firstHopOptions: string[]
}

export const routingTableStore = () =>
  createSignal(new Map<string, RoutingTableEntry>())
