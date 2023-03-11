import type { EffectContext } from "@dassie/lib-reactive"

import { subnetBalanceMapStore } from "../balances/stores/subnet-balance-map"
import { configSignal } from "../config"
import { runPerSubnetEffects } from "../peer-protocol/run-per-subnet-effects"
import { peerTableStore } from "../peer-protocol/stores/peer-table"
import modules from "./modules"
import { activeSubnetsSignal } from "./signals/active-subnets"
import { subnetMapSignal } from "./signals/subnet-map"

export const manageSubnetInstances = async (sig: EffectContext) => {
  await Promise.all(sig.for(activeSubnetsSignal, runSubnetModule))
}

const runSubnetModule = async (sig: EffectContext, subnetId: string) => {
  const realm = sig.get(configSignal, (state) => state.realm)
  const subnetMap = sig.get(subnetMapSignal)
  const subnetBalanceMap = sig.use(subnetBalanceMapStore)

  const subnetState = subnetMap.get(subnetId)

  const module = modules[subnetId]
  if (!module) {
    throw new Error(`Unknown subnet module '${subnetId}'`)
  }

  if (realm !== module.realm) {
    throw new Error("Subnet module is not compatible with realm")
  }

  if (subnetState?.initialPeers) {
    for (const peer of subnetState.initialPeers) {
      sig.use(peerTableStore).addPeer({
        subnetId,
        ...peer,
        nodePublicKey: Buffer.from(peer.nodePublicKey, "hex"),
        state: { id: "request-peering" },
      })
    }
  }

  subnetBalanceMap.setBalance(subnetId, 0n)

  /**
   * Instantiate subnet specific effects.
   */
  await sig.run(module.effect, { subnetId })

  /**
   * Instantiate aspects of the peer protocol that are specific to this subnet.
   */
  await sig.run(runPerSubnetEffects, {
    subnetId,
  })

  sig.onCleanup(() => {
    subnetBalanceMap.clearBalance(subnetId)
  })
}
