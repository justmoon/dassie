import { initTRPC } from "@trpc/server"
import superjson from "superjson"
import { z } from "zod"

import { createLogger } from "@dassie/lib-logger"
import { subscribeToSignal } from "@dassie/lib-reactive-trpc/server"

import { environmentConfigSignal, nodeTableStore } from ".."
import { totalOwnerBalanceComputed } from "../accounting/computed/total-owner-balance"
import { ledgerStore } from "../accounting/stores/ledger"
import { routingTableSignal } from "../ilp-connector/signals/routing-table"
import { spspPaymentQueueStore } from "../spsp-server/send-spsp-payments"
import { resolvePaymentPointer } from "../utils/resolve-payment-pointer"
import type { TrpcContext } from "./trpc-context"

const logger = createLogger("das:node:trpc-router")

const trpc = initTRPC.context<TrpcContext>().create({ transformer: superjson })

export const appRouter = trpc.router({
  resolvePaymentPointer: trpc.procedure
    .input(
      z.object({
        paymentPointer: z.string(),
      })
    )
    .query(async ({ input: { paymentPointer } }) => {
      return resolvePaymentPointer(paymentPointer)
    }),
  createPayment: trpc.procedure
    .input(
      z.object({
        paymentId: z.string(),
        paymentPointer: z.string(),
        amount: z.string(),
      })
    )
    .mutation(
      ({ input: { paymentId, paymentPointer, amount }, ctx: { sig } }) => {
        // TODO: Validate paymentId length
        // TODO: Verify paymentId is unique
        logger.debug("creating payment", { paymentPointer, amount })
        const spspPaymentQueue = sig.use(spspPaymentQueueStore)
        spspPaymentQueue.addPayment({
          id: paymentId,
          destination: paymentPointer,
          totalAmount: BigInt(amount),
          sentAmount: BigInt(0),
        })
      }
    ),
  subscribeBalance: trpc.procedure.subscription(({ ctx: { sig } }) => {
    return subscribeToSignal(sig, totalOwnerBalanceComputed)
  }),
  getLedger: trpc.procedure.query(({ ctx: { sig } }) => {
    return [...sig.use(ledgerStore).getAccounts("")]
  }),
  subscribeConfig: trpc.procedure.subscription(({ ctx: { sig } }) => {
    return subscribeToSignal(sig, environmentConfigSignal)
  }),
  subscribeNodeTable: trpc.procedure.subscription(({ ctx: { sig } }) => {
    return subscribeToSignal(sig, nodeTableStore)
  }),
  subscribeRoutingTable: trpc.procedure.subscription(({ ctx: { sig } }) => {
    return subscribeToSignal(sig, routingTableSignal)
  }),
})

export type AppRouter = typeof appRouter
