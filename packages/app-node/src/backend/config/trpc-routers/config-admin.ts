import { z } from "zod"

import { createRouter } from "@dassie/lib-rpc/server"

import { SetupUrlSignal } from "../../authentication/computed/setup-url"
import { VALID_REALMS } from "../../constants/general"
import { NodeIdSignal } from "../../ilp-connector/computed/node-id"
import { SettlementSchemeId } from "../../peer-protocol/types/settlement-scheme-id"
import { protectedRoute } from "../../rpc-server/route-types/protected"
import { SettlementSchemesStore } from "../../settlement-schemes/database-stores/settlement-schemes"
import { HasTlsSignal } from "../computed/has-tls"
import { DatabaseConfigStore } from "../database-config"

export const configAdminRouter = createRouter({
  getConfig: protectedRoute.query(({ context: { sig } }) => {
    const { hostname, dassieKey } = sig.read(DatabaseConfigStore)

    const nodeIdFields =
      dassieKey === undefined ?
        {
          hasNodeIdentity: false as const,
        }
      : {
          hasNodeIdentity: true as const,
          nodeId: sig.read(NodeIdSignal),
        }

    const result = {
      hostname,
      ...nodeIdFields,
      hasTls: sig.read(HasTlsSignal),
    }

    return result
  }),

  getSetupUrl: protectedRoute.query(({ context: { sig } }) => {
    return sig.read(SetupUrlSignal)
  }),

  setRealm: protectedRoute
    .input(
      z.object({
        realm: z.enum(VALID_REALMS),
      }),
    )
    .mutation(({ input: { realm }, context: { sig } }) => {
      if (realm === "live") {
        throw new Error("Livenet is not yet supported")
      }

      const config = sig.reactor.use(DatabaseConfigStore)

      config.setRealm(realm)
      return true
    }),
  setHostname: protectedRoute
    .input(
      z.object({
        hostname: z.string(),
      }),
    )
    .mutation(({ input: { hostname }, context: { sig } }) => {
      const config = sig.reactor.use(DatabaseConfigStore)

      config.setHostname(hostname)
      return true
    }),
  addSettlementScheme: protectedRoute
    .input(
      z.object({
        id: z.string().transform((id) => id as SettlementSchemeId),
        config: z.object({}),
      }),
    )
    .mutation(({ context: { sig }, input: { id, config } }) => {
      sig.reactor.use(SettlementSchemesStore).addSettlementScheme(id, config)
    }),
})
