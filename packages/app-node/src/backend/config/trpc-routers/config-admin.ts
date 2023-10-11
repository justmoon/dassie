import { z } from "zod"

import { SetupUrlSignal } from "../../authentication/computed/setup-url"
import { VALID_REALMS } from "../../constants/general"
import { NodeIdSignal } from "../../ilp-connector/computed/node-id"
import { trpc } from "../../local-ipc-server/trpc-context"
import { SettlementSchemeId } from "../../peer-protocol/types/settlement-scheme-id"
import { SettlementSchemesStore } from "../../settlement-schemes/database-stores/settlement-schemes"
import { protectedProcedure } from "../../trpc-server/middlewares/auth"
import { HasTlsSignal } from "../computed/has-tls"
import { DatabaseConfigStore } from "../database-config"

export const configAdminRouter = trpc.router({
  getConfig: protectedProcedure.query(({ ctx: { sig } }) => {
    const { hostname, dassieKey } = sig.use(DatabaseConfigStore).read()

    const nodeIdFields =
      dassieKey === undefined
        ? {
            hasNodeIdentity: false as const,
          }
        : {
            hasNodeIdentity: true as const,
            nodeId: sig.use(NodeIdSignal).read(),
          }

    const result = {
      hostname,
      ...nodeIdFields,
      hasTls: sig.use(HasTlsSignal).read(),
    }

    return result
  }),

  getSetupUrl: protectedProcedure.query(({ ctx: { sig } }) => {
    const setupUrl = sig.use(SetupUrlSignal).read()
    return setupUrl
  }),

  setRealm: protectedProcedure
    .input(
      z.object({
        realm: z.enum(VALID_REALMS),
      }),
    )
    .mutation(({ input: { realm }, ctx: { sig } }) => {
      if (realm === "live") {
        throw new Error("Livenet is not yet supported")
      }

      const config = sig.use(DatabaseConfigStore)

      config.setRealm(realm)
      return true
    }),
  setHostname: protectedProcedure
    .input(
      z.object({
        hostname: z.string(),
      }),
    )
    .mutation(({ input: { hostname }, ctx: { sig } }) => {
      const config = sig.use(DatabaseConfigStore)

      config.setHostname(hostname)
      return true
    }),
  addSettlementScheme: protectedProcedure
    .input(
      z.object({
        id: z.string().transform((id) => id as SettlementSchemeId),
        config: z.object({}),
      }),
    )
    .mutation(({ ctx: { sig }, input: { id, config } }) => {
      sig.use(SettlementSchemesStore).addSettlementScheme(id, config)
    }),
})
