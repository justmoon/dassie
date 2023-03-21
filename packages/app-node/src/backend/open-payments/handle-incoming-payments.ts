import { nanoid } from "nanoid"
import { z } from "zod"

import { respondJson } from "@dassie/lib-http-server"
import { createActor } from "@dassie/lib-reactive"

import { configSignal } from "../config"
import { databaseSignal } from "../database/open-database"
import { restApiService } from "../http-server/serve-rest-api"
import { streamServerService } from "../spsp-server/stream-server"
import { PAYMENT_POINTER_ROOT } from "./constants/payment-pointer"
import { createIncomingPaymentFormatter } from "./utils/format-incoming-payment"

export const handleIncomingPayments = () =>
  createActor(async (sig) => {
    const api = sig.get(restApiService)
    const database = sig.get(databaseSignal)
    const streamServer = await sig.get(streamServerService)
    const { url } = sig.getKeys(configSignal, ["url"])

    if (!api || !streamServer) return

    const formatIncomingPayment = createIncomingPaymentFormatter({
      url,
      streamServer,
    })

    // List incoming payments
    {
      const { dispose } = api
        .get(`${PAYMENT_POINTER_ROOT}/incoming-payments`)
        .cors()
        .handler((_request, response) => {
          const payments = database.tables.incomingPayment.selectAll()

          respondJson(response, 200, {
            result: payments.map((payment) => formatIncomingPayment(payment)),
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
          const payment = {
            id,
            subnet: "null",
            total_amount: BigInt(incomingAmount.value),
            received_amount: 0n,
            external_reference: externalRef ?? "",
          }

          database.tables.incomingPayment.insertOne(payment)

          respondJson(response, 200, formatIncomingPayment(payment))
        })
      sig.onCleanup(dispose)
    }
  })
