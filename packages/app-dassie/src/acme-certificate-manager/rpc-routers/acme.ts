import { z } from "zod"

import { createRouter } from "@dassie/lib-rpc/server"

import { Database } from "../../database/open-database"
import { protectedRoute } from "../../rpc-server/route-types/protected"

export const acmeRouter = createRouter({
  setAcmeCredentials: protectedRoute
    .input(
      z.object({
        accountUrl: z.string(),
        accountKey: z.string(),
      }),
    )
    .mutation(({ input: { accountUrl, accountKey }, context: { sig } }) => {
      const database = sig.reactor.use(Database)
      database.scalars.acmeAccountUrl.set(accountUrl)
      database.scalars.acmeAccountKey.set(accountKey)
      return true
    }),
  registerToken: protectedRoute
    .input(
      z.object({
        token: z.string(),
        keyAuthorization: z.string(),
        expires: z.coerce.date().optional(),
      }),
    )
    .mutation(
      ({ input: { token, keyAuthorization, expires }, context: { sig } }) => {
        const database = sig.reactor.use(Database)

        database.tables.acmeTokens.insertOne({
          token,
          key_authorization: keyAuthorization,
          expires: expires?.toISOString() ?? null,
        })
      },
    ),
  deregisterToken: protectedRoute
    .input(
      z.object({
        token: z.string(),
      }),
    )
    .mutation(({ input: { token }, context: { sig } }) => {
      const database = sig.reactor.use(Database)

      database.tables.acmeTokens.delete({
        token,
      })
    }),
})
