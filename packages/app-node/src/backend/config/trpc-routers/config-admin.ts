import { z } from "zod"

import { setupUrlComputed } from "../../authentication/computed/setup-url"
import { VALID_REALMS } from "../../constants/general"
import { nodeIdSignal } from "../../ilp-connector/computed/node-id"
import { trpc } from "../../local-ipc-server/trpc-context"
import { SettlementSchemeId } from "../../peer-protocol/types/settlement-scheme-id"
import { settlementSchemesStore } from "../../settlement-schemes/database-stores/settlement-schemes"
import { protectedProcedure } from "../../trpc-server/middlewares/auth"
import { hasTlsComputed } from "../computed/has-tls"
import { databaseConfigStore } from "../database-config"

export const configAdminRouter = trpc.router({
  getConfig: protectedProcedure.query(({ ctx: { sig } }) => {
    const { hostname, dassieKey } = sig.use(databaseConfigStore).read()

    const nodeIdFields =
      dassieKey === undefined
        ? {
            hasNodeIdentity: false as const,
          }
        : {
            hasNodeIdentity: true as const,
            nodeId: sig.use(nodeIdSignal).read(),
          }

    const result = {
      hostname,
      ...nodeIdFields,
      hasTls: sig.use(hasTlsComputed).read(),
    }

    return result
  }),

  getSetupUrl: protectedProcedure.query(({ ctx: { sig } }) => {
    const setupUrl = sig.use(setupUrlComputed).read()
    return setupUrl
  }),

  setRealm: protectedProcedure
    .input(
      z.object({
        realm: z.enum(VALID_REALMS),
      })
    )
    .mutation(({ input: { realm }, ctx: { sig } }) => {
      if (realm === "live") {
        throw new Error("Livenet is not yet supported")
      }

      const config = sig.use(databaseConfigStore)

      config.setRealm(realm)
      return true
    }),
  setHostname: protectedProcedure
    .input(
      z.object({
        hostname: z.string(),
      })
    )
    .mutation(({ input: { hostname }, ctx: { sig } }) => {
      const config = sig.use(databaseConfigStore)

      config.setHostname(hostname)
      return true
    }),
  addSettlementScheme: protectedProcedure
    .input(
      z.object({
        id: z.string().transform((id) => id as SettlementSchemeId),
        config: z.object({}),
      })
    )
    .mutation(({ ctx: { sig }, input: { id, config } }) => {
      sig.use(settlementSchemesStore).addSettlementScheme(id, config)
    }),
})
