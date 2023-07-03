import { debugRouter } from "./routers/debug"
import { generalRouter } from "./routers/general"
import { paymentRouter } from "./routers/payment"
import { trpc } from "./trpc-context"

export const appRouter = trpc.router({
  general: generalRouter,
  payment: paymentRouter,
  debug: debugRouter,
})

export type AppRouter = typeof appRouter
