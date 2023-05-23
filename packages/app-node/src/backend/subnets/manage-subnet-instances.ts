import { hexToBytes } from "@noble/hashes/utils"

import { createActor } from "@dassie/lib-reactive"

import { configSignal } from "../config"
import { speakPeerProtocolPerSubnet } from "../peer-protocol"
import { sendPeerMessage } from "../peer-protocol/actors/send-peer-message"
import { nodeTableStore } from "../peer-protocol/stores/node-table"
import modules from "./modules"
import { registerSubnetActor } from "./register-subnet-actor"
import { activeSubnetsSignal } from "./signals/active-subnets"
import { subnetMapSignal } from "./signals/subnet-map"
import type {
  SubnetActorFactory,
  SubnetHostMethods,
} from "./types/subnet-module"

export const manageSubnetInstances = () =>
  createActor(async (sig) => {
    await Promise.all(sig.for(activeSubnetsSignal, runSubnetModule))
  })

export interface PerSubnetParameters {
  subnetId: string
  subnetActor: SubnetActorFactory
}

const runSubnetModule = () =>
  createActor(async (sig, subnetId: string) => {
    const realm = sig.get(configSignal, (state) => state.realm)
    const subnetMap = sig.get(subnetMapSignal)

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
          alias: peer.alias ?? "initial",
          nodePublicKey,
          linkState: {
            sequence: 0n,
            updateReceivedCounter: 0,
            scheduledRetransmitTime: 0,
            neighbors: [],
            subnets: [],
            lastUpdate: undefined,
          },
          peerState: { id: "request-peering", lastSeen: 0, subnetId },
        })
      }
    }

    // Create a unique factory for the subnet module actor.
    //
    // The same subnet module may be used for different subnets, so we are deliberately creating a unique factory for each subnet.
    const subnetActor = () => createActor(module.behavior)

    // For easier debugging, we give the subnet factory a unique name as well.
    Object.defineProperty(subnetActor, "name", {
      value: subnetId,
      writable: false,
    })

    const host: SubnetHostMethods = {
      sendMessage: ({ peerId, message }) => {
        sig.use(sendPeerMessage).tell("send", {
          destination: peerId,
          message: {
            type: "subnetModuleMessage",
            value: {
              subnetId,
              message,
            },
          },
        })
      },
    }

    // Instantiate subnet module actor.
    await sig.run(subnetActor, { subnetId, host }, { register: true }).result

    const subnetParameters: PerSubnetParameters = {
      subnetId,
      subnetActor,
    }

    // Instantiate aspects of the peer protocol that are specific to this subnet.
    await sig.run(speakPeerProtocolPerSubnet, subnetParameters).result

    sig.run(registerSubnetActor, subnetParameters)
  })
