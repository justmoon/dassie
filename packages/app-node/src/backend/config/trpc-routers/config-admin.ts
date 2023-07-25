import { z } from "zod"

import { VALID_REALMS } from "../../constants/general"
import { databasePlain } from "../../database/open-database"
import { trpc } from "../../local-ipc-server/trpc-context"
import { protectedProcedure } from "../../trpc-server/middlewares/auth"

export const configAdminRouter = trpc.router({
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

      const database = sig.use(databasePlain)
      database.scalars.set("config.realm", realm)
      return true
    }),
})
