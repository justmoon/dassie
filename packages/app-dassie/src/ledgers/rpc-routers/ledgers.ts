import { z } from "zod"

import { createRouter } from "@dassie/lib-rpc/server"

import { PeersSignal } from "../../peer-protocol/computed/peers"
import { NodeTableStore } from "../../peer-protocol/stores/node-table"
import type { SettlementSchemeId } from "../../peer-protocol/types/settlement-scheme-id"
import { protectedRoute } from "../../rpc-server/route-types/protected"
import { SettlementSchemesStore } from "../database-stores/settlement-schemes"
import { ManageSettlementSchemeInstancesActor } from "../manage-settlement-scheme-instances"

export const ledgersRouter = createRouter({
  getList: protectedRoute.query(({ context: { sig } }) => {
    const settlementSchemes = sig.reactor.use(SettlementSchemesStore).read()
    const manageSettlementSchemeInstancesActor = sig.reactor.use(
      ManageSettlementSchemeInstancesActor,
    )

    return Promise.all(
      settlementSchemes.map(async (settlementScheme) => {
        const settlementActor = manageSettlementSchemeInstancesActor.get(
          settlementScheme.id,
        )
        return {
          id: settlementScheme.id,
          balance: (await settlementActor?.api.getBalance.ask()) ?? 0n,
        }
      }),
    )
  }),
  checkLedgerDeletePrerequisites: protectedRoute
    .input(z.string())
    .query(({ input: ledgerId, context: { sig } }) => {
      const manageSettlementSchemeInstancesActor = sig.reactor.use(
        ManageSettlementSchemeInstancesActor,
      )

      const settlementActor = manageSettlementSchemeInstancesActor.get(
        ledgerId as SettlementSchemeId,
      )

      const peers = sig.read(PeersSignal)
      const nodeTable = sig.reactor.use(NodeTableStore)

      const isUnused = ![...peers]
        .map((peerId) => nodeTable.read().get(peerId)?.peerState)
        .some(
          (peerInfo) =>
            peerInfo?.id === "peered" &&
            peerInfo.settlementSchemeId === ledgerId,
        )

      return {
        isActive: !!settlementActor,
        isUnused,
      }
    }),
  addSettlementScheme: protectedRoute
    .input(
      z.object({
        id: z.string().transform((id) => id as SettlementSchemeId),
        config: z.object({}),
      }),
    )
    .mutation(({ context: { sig }, input: { id, config } }) => {
      sig.reactor
        .use(SettlementSchemesStore)
        .act.addSettlementScheme(id, config)
    }),
  removeSettlementScheme: protectedRoute
    .input(
      z.object({
        id: z.string().transform((id) => id as SettlementSchemeId),
      }),
    )
    .mutation(({ context: { sig }, input: { id } }) => {
      sig.reactor.use(SettlementSchemesStore).act.removeSettlementScheme(id)
    }),
  stubDeposit: protectedRoute
    .input(z.string())
    .mutation(async ({ input: amount, context: { sig } }) => {
      const manageSettlementSchemeInstancesActor = sig.reactor.use(
        ManageSettlementSchemeInstancesActor,
      )

      const stubActor = manageSettlementSchemeInstancesActor.get(
        "stub" as SettlementSchemeId,
      )

      if (!stubActor) {
        throw new Error("Stub settlement scheme is not enabled")
      }

      await stubActor.api.handleDeposit.ask({ amount: BigInt(amount) })

      return true
    }),
})
