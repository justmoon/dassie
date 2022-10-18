import { nanoid } from "nanoid"
import { z } from "zod"

import { respondJson } from "@dassie/lib-http-server"
import type { EffectContext } from "@dassie/lib-reactive"

import { configSignal } from "../config"
import { IncomingPayment } from "../database/models/incoming-payment"
import { restApiService } from "../http-server/serve-rest-api"
import { streamServerService } from "../spsp-server/stream-server"

export const PAYMENT_POINTER_ROOT = "/.well-known/pay"

export const handleIncomingPayments = async (sig: EffectContext) => {
  const api = sig.get(restApiService)
  const streamServer = await sig.get(streamServerService)
  const { url } = sig.getKeys(configSignal, ["url"])

  if (!api || !streamServer) return

  // List incoming payments
  {
    const { dispose } = api
      .get(`${PAYMENT_POINTER_ROOT}/incoming-payments`)
      .cors()
      .handler((_request, response) => {
        const payments = IncomingPayment.getAll(sig)
        console.log(payments)

        respondJson(response, 200, {
          result: payments.map((payment) => ({
            id: `${url}${PAYMENT_POINTER_ROOT}/incoming-payments/${payment.data.id}`,
          })),
        })
      })
    sig.onCleanup(dispose)
  }

  // Create a new incoming payment
  {
    const { dispose } = api
      .post(`${PAYMENT_POINTER_ROOT}/incoming-payments`)
      .cors()
      .body(
        z.object({
          incomingAmount: z.object({
            value: z.string(),
            assetCode: z.string(),
            assetScale: z.number(),
          }),
          expiresAt: z.string().optional(),
          description: z.string().optional(),
          externalRef: z.string().optional(),
        })
      )
      .handler((request, response) => {
        const { incomingAmount, externalRef } = request.body
        const id = nanoid()
        const createdAt = new Date().toISOString()

        const paymentRowId = IncomingPayment.create(sig, {
          id,
          total_amount: BigInt(incomingAmount.value),
          external_reference: externalRef ?? "",
        })

        const payment = IncomingPayment.get(sig, paymentRowId)

        if (!payment) {
          throw new Error("Payment not found in database after creation")
        }

        const { destinationAccount, sharedSecret } =
          streamServer.generateAddressAndSecret({
            connectionTag: paymentRowId.toString(),
          })

        respondJson(response, 200, {
          id: `${url}${PAYMENT_POINTER_ROOT}/incoming-payments/${id}`,
          incomingAmount,
          receivedAmount: {
            value: payment.data.received_amount.toString(),
            assetCode: incomingAmount.assetCode,
            assetScale: incomingAmount.assetScale,
          },
          ilpStreamConnection: {
            id: `${url}${PAYMENT_POINTER_ROOT}/connections/${id}`,
            ilpAddress: destinationAccount,
            sharedSecret: sharedSecret.toString("base64url"),
            assetCode: incomingAmount.assetCode,
            assetScale: incomingAmount.assetScale,
          },
          createdAt,
          updatedAt: createdAt,
        })
      })
    sig.onCleanup(dispose)
  }
}
