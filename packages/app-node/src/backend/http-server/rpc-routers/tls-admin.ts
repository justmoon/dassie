import { z } from "zod"

import { createRouter } from "@dassie/lib-rpc/server"

import { DatabaseConfigStore } from "../../config/database-config"
import { protectedRoute } from "../../rpc-server/route-types/protected"

export const tlsAdminRouter = createRouter({
  setNodeTlsConfiguration: protectedRoute
    .input(
      z.object({
        certificate: z.string(),
        privateKey: z.string(),
      }),
    )
    .mutation(({ input: { certificate, privateKey }, context: { sig } }) => {
      const config = sig.reactor.use(DatabaseConfigStore)
      config.act.setTlsCertificates(certificate, privateKey)

      return true
    }),
})
