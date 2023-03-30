import { hexToBytes } from "@noble/hashes/utils"

import { createActor } from "@dassie/lib-reactive"

import { subnetBalanceMapStore } from "../balances/stores/subnet-balance-map"
import { configSignal } from "../config"
import { runPerSubnetEffects } from "../peer-protocol/run-per-subnet-effects"
import { nodeTableStore } from "../peer-protocol/stores/node-table"
import modules from "./modules"
import { activeSubnetsSignal } from "./signals/active-subnets"
import { subnetMapSignal } from "./signals/subnet-map"

export const manageSubnetInstances = () =>
  createActor(async (sig) => {
    await Promise.all(sig.for(activeSubnetsSignal, runSubnetModule))
  })

const runSubnetModule = () =>
  createActor(async (sig, subnetId: string) => {
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
        const nodePublicKey = hexToBytes(peer.nodePublicKey)
        sig.use(nodeTableStore).addNode({
          ...peer,
          nodePublicKey,
          subnetId,
          linkState: {
            sequence: 0n,
            updateReceivedCounter: 0,
            scheduledRetransmitTime: 0,
            neighbors: [],
            lastUpdate: undefined,
          },
          peerState: { id: "request-peering", lastSeen: 0 },
        })
      }
    }

    subnetBalanceMap.setBalance(subnetId, 0n)

    /**
     * Instantiate subnet specific effects.
     */
    await sig.run(module.actor, { subnetId })

    /**
     * Instantiate aspects of the peer protocol that are specific to this subnet.
     */
    await sig.run(runPerSubnetEffects, {
      subnetId,
    })

    sig.onCleanup(() => {
      subnetBalanceMap.clearBalance(subnetId)
    })
  })
