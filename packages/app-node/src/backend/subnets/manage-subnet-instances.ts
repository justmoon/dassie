import { hexToBytes } from "@noble/hashes/utils"

import { createActor } from "@dassie/lib-reactive"

import { initializeCommonAccounts } from "../accounting/functions/manage-common-accounts"
import { ledgerStore } from "../accounting/stores/ledger"
import { configSignal } from "../config"
import { speakPeerProtocolPerSubnet } from "../peer-protocol"
import { sendPeerMessage } from "../peer-protocol/actors/send-peer-message"
import { nodeTableStore } from "../peer-protocol/stores/node-table"
import { NodeId } from "../peer-protocol/types/node-id"
import { SubnetId } from "../peer-protocol/types/subnet-id"
import modules from "./modules"
import { activeSubnetsSignal } from "./signals/active-subnets"
import { subnetMapSignal } from "./signals/subnet-map"
import type { SubnetActor, SubnetHostMethods } from "./types/subnet-module"

export interface SubnetInstance {
  actor: SubnetActor
}

export const manageSubnetInstances = () =>
  createActor(async (sig) => {
    const subnetActorMap = new Map<SubnetId, SubnetInstance>()
    await Promise.all(
      sig.for(activeSubnetsSignal, () =>
        createActor(async (sig, subnetId) => {
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

          const ledger = sig.use(ledgerStore)
          initializeCommonAccounts(ledger, subnetId)

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
          const subnetActorFactory = () => createActor(module.behavior)

          // For easier debugging, we give the subnet factory a unique name as well.
          Object.defineProperty(subnetActorFactory, "name", {
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
          const subnetActor = sig.run(
            subnetActorFactory,
            { subnetId, host },
            { register: true }
          )

          subnetActorMap.set(subnetId, {
            actor: subnetActor,
          })

          await subnetActor.result

          const subnetParameters: PerSubnetParameters = {
            subnetId,
            subnetActor,
          }

          // Instantiate aspects of the peer protocol that are specific to this subnet.
          await sig.run(speakPeerProtocolPerSubnet, subnetParameters).result

          sig.onCleanup(() => {
            subnetActorMap.delete(subnetId)
          })
        })
      )
    )

    return {
      getActor: (subnetId: SubnetId) => subnetActorMap.get(subnetId)?.actor,
      handleMessage: ({
        subnetId,
        peerId,
        message,
      }: {
        subnetId: SubnetId
        peerId: NodeId
        message: Uint8Array
      }) => {
        const subnetInstance = subnetActorMap.get(subnetId)

        if (subnetInstance) {
          subnetInstance.actor.tell("handleMessage", {
            peerId,
            message,
          })
        }
      },
    }
  })

export interface PerSubnetParameters {
  subnetId: SubnetId
  subnetActor: SubnetActor
}
