import { Reactor, createActor, createMapped } from "@dassie/lib-reactive"

import { initializeCommonAccounts } from "../accounting/functions/manage-common-accounts"
import { LedgerStore } from "../accounting/stores/ledger"
import { DatabaseConfigStore } from "../config/database-config"
import { SendPeerMessageActor } from "../peer-protocol/actors/send-peer-message"
import modules from "./modules"
import { ActiveSettlementSchemesSignal } from "./signals/active-settlement-schemes"
import type {
  SettlementSchemeActor,
  SettlementSchemeHostMethods,
} from "./types/settlement-scheme-module"

export interface SettlementSchemeInstance {
  actor: SettlementSchemeActor
}

export const ManageSettlementSchemeInstancesActor = (reactor: Reactor) => {
  const ledgerStore = reactor.use(LedgerStore)

  return createMapped(
    reactor.lifecycle,
    reactor.use(ActiveSettlementSchemesSignal),
    (settlementSchemeId) =>
      createActor(async (sig) => {
        const { realm } = sig.getKeys(DatabaseConfigStore, ["realm"])

        const module = modules[settlementSchemeId]
        if (!module) {
          throw new Error(`Unknown settlement scheme '${settlementSchemeId}'`)
        }

        if (realm !== module.realm) {
          throw new Error("Subnet module is not compatible with realm")
        }

        initializeCommonAccounts(ledgerStore, settlementSchemeId)

        // Create a unique factory for the settlement scheme actor.
        //
        // The same settlement scheme module may be used for different settlement schemes, so we are deliberately creating a unique factory for each settlement scheme.
        const SettlementSchemeActor = () => createActor(module.behavior)

        // For easier debugging, we give the settlement scheme factory a unique name as well.
        Object.defineProperty(SettlementSchemeActor, "name", {
          value: settlementSchemeId,
          writable: false,
        })

        const host: SettlementSchemeHostMethods = {
          sendMessage: ({ peerId, message }) => {
            sig.use(SendPeerMessageActor).api.send.tell({
              destination: peerId,
              message: {
                type: "settlementMessage",
                value: {
                  settlementSchemeId,
                  message,
                },
              },
            })
          },
        }

        // Instantiate settlement scheme module actor.
        const settlementSchemeActor = sig.use(SettlementSchemeActor)
        const settlementSchemeActorMethods = await settlementSchemeActor.run(
          sig.reactor,
          sig,
          {
            settlementSchemeId,
            host,
          },
        )

        if (!settlementSchemeActorMethods) {
          throw new Error("Subnet module failed to initialize")
        }

        return settlementSchemeActorMethods
      }),
  )
}
