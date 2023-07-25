import { z } from "zod"

import { databasePlain } from "../../database/open-database"
import { trpc } from "../../local-ipc-server/trpc-context"
import { protectedProcedure } from "../../trpc-server/middlewares/auth"

export const tlsAdminRouter = trpc.router({
  setNodeTlsConfiguration: protectedProcedure
    .input(
      z.object({
        certificate: z.string(),
        privateKey: z.string(),
      })
    )
    .mutation(({ input: { certificate, privateKey }, ctx: { sig } }) => {
      const database = sig.use(databasePlain)
      database.scalars.set("config.tls_web_cert", certificate)
      database.scalars.set("config.tls_web_key", privateKey)
      return true
    }),
})
