import { z } from "zod"

import { createLogger } from "@dassie/lib-logger"

import { spspPaymentQueueStore } from "../../spsp-server/send-spsp-payments"
import { resolvePaymentPointer } from "../../utils/resolve-payment-pointer"
import { trpc } from "../trpc-context"

const logger = createLogger("das:trpc-router:payment")

export const paymentRouter = trpc.router({
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
})
