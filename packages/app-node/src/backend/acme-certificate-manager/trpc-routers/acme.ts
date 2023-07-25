import { z } from "zod"

import { databasePlain } from "../../database/open-database"
import { trpc } from "../../local-ipc-server/trpc-context"
import { protectedProcedure } from "../../trpc-server/middlewares/auth"

export const acmeRouter = trpc.router({
  setAcmeCredentials: protectedProcedure
    .input(
      z.object({
        accountUrl: z.string(),
        accountKey: z.string(),
      })
    )
    .mutation(({ input: { accountUrl, accountKey }, ctx: { sig } }) => {
      const database = sig.use(databasePlain)
      database.scalars.set("acme.account_url", accountUrl)
      database.scalars.set("acme.account_key", accountKey)
      return true
    }),
  registerToken: protectedProcedure
    .input(
      z.object({
        token: z.string(),
        keyAuthorization: z.string(),
        expires: z.coerce.date().optional(),
      })
    )
    .mutation(
      ({ input: { token, keyAuthorization, expires }, ctx: { sig } }) => {
        const database = sig.use(databasePlain)

        database.tables.acmeTokens.insertOne({
          token,
          key_authorization: keyAuthorization,
          expires: expires ?? null,
        })
      }
    ),
  deregisterToken: protectedProcedure
    .input(
      z.object({
        token: z.string(),
      })
    )
    .mutation(({ input: { token }, ctx: { sig } }) => {
      const database = sig.use(databasePlain)

      database.tables.acmeTokens.select().where({ equals: { token } }).delete()
    }),
})
