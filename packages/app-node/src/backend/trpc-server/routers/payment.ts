import { z } from "zod"

import { payment as logger } from "../../logger/instances"
import { SpspPaymentQueueStore } from "../../spsp-server/send-spsp-payments"
import { resolvePaymentPointer } from "../../utils/resolve-payment-pointer"
import { protectedProcedure } from "../middlewares/auth"
import { trpc } from "../trpc-context"

export const paymentRouter = trpc.router({
  resolvePaymentPointer: protectedProcedure
    .input(
      z.object({
        paymentPointer: z.string(),
      }),
    )
    .query(async ({ input: { paymentPointer } }) => {
      return resolvePaymentPointer(paymentPointer)
    }),
  createPayment: protectedProcedure
    .input(
      z.object({
        paymentId: z.string(),
        paymentPointer: z.string(),
        amount: z.string(),
      }),
    )
    .mutation(
      ({ input: { paymentId, paymentPointer, amount }, ctx: { sig } }) => {
        // TODO: Validate paymentId length
        // TODO: Verify paymentId is unique
        logger.debug("creating payment", { paymentPointer, amount })
        const spspPaymentQueue = sig.use(SpspPaymentQueueStore)
        spspPaymentQueue.addPayment({
          id: paymentId,
          destination: paymentPointer,
          totalAmount: BigInt(amount),
          sentAmount: BigInt(0),
        })
      },
    ),
})
