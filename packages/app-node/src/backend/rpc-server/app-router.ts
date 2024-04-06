import { createRouter } from "@dassie/lib-rpc/server"

import { acmeRouter } from "../acme-certificate-manager/trpc-routers/acme"
import { apiKeysRouter } from "../api-keys/trpc-routers/api-keys"
import { configAdminRouter } from "../config/trpc-routers/config-admin"
import { tlsAdminRouter } from "../http-server/trpc-routers/tls-admin"
import { settlementRouter } from "../settlement-schemes/trpc-routers/settlement"
import { debugRouter } from "./routers/debug"
import { generalRouter } from "./routers/general"
import { paymentRouter } from "./routers/payment"

export const appRouter = createRouter({
  general: generalRouter,
  payment: paymentRouter,
  debug: debugRouter,
  acme: acmeRouter,
  tls: tlsAdminRouter,
  config: configAdminRouter,
  apiKeys: apiKeysRouter,
  settlement: settlementRouter,
})

export type AppRouter = typeof appRouter
