import { createStore } from "@xen-ilp/lib-reactive"

export interface NodeTableEntry {
  nodeId: string
}

export type Model = Map<string, NodeTableEntry>

export const nodeTableStore = () => createStore<Model>(new Map())
