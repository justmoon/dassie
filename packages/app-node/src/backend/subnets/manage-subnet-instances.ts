import type { EffectContext } from "@dassie/lib-reactive"

import { configSignal } from "../config"
import { peerTableStore } from "../peer-protocol/stores/peer-table"
import * as modules from "./modules"
import { activeSubnetsSignal } from "./signals/active-subnets"
import { subnetMapSignal } from "./signals/subnet-map"
import type { Subnet } from "./types/subnet-module"

export const manageSubnetInstances = async (sig: EffectContext) => {
  await Promise.all(sig.for(activeSubnetsSignal, runSubnetModule))
}

const runSubnetModule = async (sig: EffectContext, subnetId: string) => {
  const realm = sig.get(configSignal, (state) => state.realm)
  const subnetMap = sig.get(subnetMapSignal)

  const subnetState = subnetMap.get(subnetId)

  const createModule = (modules as Record<string, Subnet>)[subnetId]
  if (!createModule) {
    throw new Error(`Unknown subnet module '${subnetId}'`)
  }

  const instance = await createModule()

  if (realm !== instance.realm) {
    throw new Error("Subnet module is not compatible with realm")
  }

  if (subnetState?.initialPeers) {
    for (const peer of subnetState.initialPeers) {
      sig.use(peerTableStore).addPeer({ subnetId, ...peer })
    }
  }
}
