import { initTRPC } from "@trpc/server"
import { observable } from "@trpc/server/observable"
import { z } from "zod"

import { createLogger } from "@dassie/lib-logger"

import { overallBalanceSignal } from "../balances/signals/overall-balance-signal"
import { spspPaymentQueueStore } from "../spsp-server/send-spsp-payments"
import { resolvePaymentPointer } from "../utils/resolve-payment-pointer"
import type { TrpcContext } from "./trpc-context"

const logger = createLogger("das:node:trpc-router")

const trpc = initTRPC.context<TrpcContext>().create()

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
    return observable<string>((emit) => {
      const overallBalance = sig.use(overallBalanceSignal)
      emit.next(overallBalance.read().toString())
      return overallBalance.on((balance) => {
        emit.next(balance.toString())
      })
    })
  }),
})

export type AppRouter = typeof appRouter
