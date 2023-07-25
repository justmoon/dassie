import { acmeRouter } from "../acme-certificate-manager/trpc-routers/acme"
import { configAdminRouter } from "../config/trpc-routers/config-admin"
import { tlsAdminRouter } from "../http-server/trpc-routers/tls-admin"
import { debugRouter } from "./routers/debug"
import { generalRouter } from "./routers/general"
import { paymentRouter } from "./routers/payment"
import { trpc } from "./trpc-context"

export const appRouter = trpc.router({
  general: generalRouter,
  payment: paymentRouter,
  debug: debugRouter,
  acme: acmeRouter,
  tls: tlsAdminRouter,
  config: configAdminRouter,
})

export type AppRouter = typeof appRouter
