import { hexToBytes } from "@noble/hashes/utils"

import assert from "node:assert"

import { createActor, createMapped } from "@dassie/lib-reactive"

import { initializeCommonAccounts } from "../accounting/functions/manage-common-accounts"
import { ledgerStore } from "../accounting/stores/ledger"
import { databaseConfigSignal } from "../config/database-config"
import { sendPeerMessage } from "../peer-protocol/actors/send-peer-message"
import { nodeTableStore } from "../peer-protocol/stores/node-table"
import modules from "./modules"
import { activeSettlementSchemesSignal } from "./signals/active-settlement-schemes"
import { settlementSchemeMapSignal } from "./signals/settlement-scheme-map"
import type {
  SettlementSchemeActor,
  SettlementSchemeHostMethods,
} from "./types/settlement-scheme-module"

export interface SettlementSchemeInstance {
  actor: SettlementSchemeActor
}

export const manageSettlementSchemeInstances = () =>
  createMapped(activeSettlementSchemesSignal, (settlementSchemeId) =>
    createActor(async (sig) => {
      const config = sig.get(databaseConfigSignal)

      assert(config.hasWebUi, "Web UI is not configured")

      const realm = config.realm
      const settlementSchemeMap = sig.get(settlementSchemeMapSignal)

      const settlementSchemeState = settlementSchemeMap.get(settlementSchemeId)

      const module = modules[settlementSchemeId]
      if (!module) {
        throw new Error(`Unknown settlement scheme '${settlementSchemeId}'`)
      }

      if (realm !== module.realm) {
        throw new Error("Subnet module is not compatible with realm")
      }

      const ledger = sig.use(ledgerStore)
      initializeCommonAccounts(ledger, settlementSchemeId)

      if (settlementSchemeState?.initialPeers) {
        for (const peer of settlementSchemeState.initialPeers) {
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
              settlementSchemes: [],
              lastUpdate: undefined,
            },
            peerState: {
              id: "request-peering",
              lastSeen: 0,
              settlementSchemeId,
            },
          })
        }
      }

      // Create a unique factory for the settlement scheme actor.
      //
      // The same settlement scheme module may be used for different settlement schemes, so we are deliberately creating a unique factory for each settlement scheme.
      const settlementSchemeActorFactory = () => createActor(module.behavior)

      // For easier debugging, we give the settlement scheme factory a unique name as well.
      Object.defineProperty(settlementSchemeActorFactory, "name", {
        value: settlementSchemeId,
        writable: false,
      })

      const host: SettlementSchemeHostMethods = {
        sendMessage: ({ peerId, message }) => {
          sig.use(sendPeerMessage).tell("send", {
            destination: peerId,
            message: {
              type: "settlementSchemeModuleMessage",
              value: {
                settlementSchemeId,
                message,
              },
            },
          })
        },
      }

      // Instantiate settlement scheme module actor.
      const settlementSchemeActor = sig.use(settlementSchemeActorFactory)
      const settlementSchemeActorMethods = await settlementSchemeActor.run(
        sig,
        {
          settlementSchemeId,
          host,
        }
      )

      if (!settlementSchemeActorMethods) {
        throw new Error("Subnet module failed to initialize")
      }

      return settlementSchemeActorMethods
    })
  )
