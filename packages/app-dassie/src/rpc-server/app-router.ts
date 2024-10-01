import { createRouter } from "@dassie/lib-rpc/server"

import { acmeRouter } from "../acme-certificate-manager/rpc-routers/acme"
import { apiKeysRouter } from "../api-keys/rpc-routers/api-keys"
import { configAdminRouter } from "../config/rpc-routers/config-admin"
import { tlsAdminRouter } from "../http-server/rpc-routers/tls-admin"
import { settlementRouter } from "../ledgers/rpc-routers/settlement"
import { registrationClientRouter } from "../registration-client/rpc-router/registration-client"
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
  registrationClient: registrationClientRouter,
  settlement: settlementRouter,
})

export type AppRouter = typeof appRouter
