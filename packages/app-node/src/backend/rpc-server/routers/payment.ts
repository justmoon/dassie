import { z } from "zod"

import { createRouter } from "@dassie/lib-rpc/server"

import { payment as logger } from "../../logger/instances"
import { SpspPaymentQueueStore } from "../../spsp-server/send-spsp-payments"
import { resolvePaymentPointer } from "../../utils/resolve-payment-pointer"
import { protectedRoute } from "../route-types/protected"

export const paymentRouter = createRouter({
  resolvePaymentPointer: protectedRoute
    .input(
      z.object({
        paymentPointer: z.string(),
      }),
    )
    .query(async ({ input: { paymentPointer } }) => {
      return resolvePaymentPointer(paymentPointer)
    }),
  createPayment: protectedRoute
    .input(
      z.object({
        paymentId: z.string(),
        paymentPointer: z.string(),
        amount: z.string(),
      }),
    )
    .mutation(
      ({ input: { paymentId, paymentPointer, amount }, context: { sig } }) => {
        // TODO: Validate paymentId length
        // TODO: Verify paymentId is unique
        logger.debug?.("creating payment", { paymentPointer, amount })
        const spspPaymentQueue = sig.reactor.use(SpspPaymentQueueStore)
        spspPaymentQueue.addPayment({
          id: paymentId,
          destination: paymentPointer,
          totalAmount: BigInt(amount),
          sentAmount: BigInt(0),
        })
      },
    ),
})
