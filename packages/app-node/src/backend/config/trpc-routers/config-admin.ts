import { z } from "zod"

import { VALID_REALMS } from "../../constants/general"
import { trpc } from "../../local-ipc-server/trpc-context"
import { protectedProcedure } from "../../trpc-server/middlewares/auth"
import { databaseConfigStore } from "../database-config"

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

      const config = sig.use(databaseConfigStore)

      config.setRealm(realm)
      return true
    }),
})
