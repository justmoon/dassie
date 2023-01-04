import { createSignal } from "@dassie/lib-reactive"

export interface SubnetState {
  subnetId: string
  config: unknown
  initialPeers: { nodeId: string; url: string; nodePublicKey: string }[]
}

export const subnetMapSignal = () =>
  createSignal<Map<string, SubnetState>>(new Map())
