import { createSignal } from "@dassie/lib-reactive"

import { NodeId } from "../../peer-protocol/types/node-id"
import { SubnetId } from "../../peer-protocol/types/subnet-id"

export interface SubnetState {
  subnetId: SubnetId
  config: unknown
  initialPeers: {
    nodeId: NodeId
    url: string
    alias?: string | undefined
    nodePublicKey: string
  }[]
}

export const subnetMapSignal = () =>
  createSignal<Map<string, SubnetState>>(new Map())
