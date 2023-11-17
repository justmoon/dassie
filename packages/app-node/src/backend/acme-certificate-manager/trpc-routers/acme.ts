import { z } from "zod"

import { Database } from "../../database/open-database"
import { trpc } from "../../local-ipc-server/trpc-context"
import { protectedProcedure } from "../../trpc-server/middlewares/auth"

export const acmeRouter = trpc.router({
  setAcmeCredentials: protectedProcedure
    .input(
      z.object({
        accountUrl: z.string(),
        accountKey: z.string(),
      }),
    )
    .mutation(({ input: { accountUrl, accountKey }, ctx: { sig } }) => {
      const database = sig.reactor.use(Database)
      database.scalars.acmeAccountUrl.set(accountUrl)
      database.scalars.acmeAccountKey.set(accountKey)
      return true
    }),
  registerToken: protectedProcedure
    .input(
      z.object({
        token: z.string(),
        keyAuthorization: z.string(),
        expires: z.coerce.date().optional(),
      }),
    )
    .mutation(
      ({ input: { token, keyAuthorization, expires }, ctx: { sig } }) => {
        const database = sig.reactor.use(Database)

        database.tables.acmeTokens.insertOne({
          token,
          key_authorization: keyAuthorization,
          expires: expires?.toISOString() ?? null,
        })
      },
    ),
  deregisterToken: protectedProcedure
    .input(
      z.object({
        token: z.string(),
      }),
    )
    .mutation(({ input: { token }, ctx: { sig } }) => {
      const database = sig.reactor.use(Database)

      database.tables.acmeTokens.delete({
        token,
      })
    }),
})
