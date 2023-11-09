import { nanoid } from "nanoid"
import { z } from "zod"

import { createJsonResponse } from "@dassie/lib-http-server"
import { createActor } from "@dassie/lib-reactive"

import { DatabaseConfigStore } from "../config/database-config"
import { Database } from "../database/open-database"
import { RestApiServiceActor } from "../http-server/serve-rest-api"
import { StreamServerServiceActor } from "../spsp-server/stream-server"
import { PAYMENT_POINTER_ROOT } from "./constants/payment-pointer"
import { createIncomingPaymentFormatter } from "./utils/format-incoming-payment"

export const HandleIncomingPaymentsActor = () =>
  createActor((sig) => {
    const api = sig.get(RestApiServiceActor)
    const database = sig.use(Database)
    const streamServer = sig.get(StreamServerServiceActor)
    const { url } = sig.get(DatabaseConfigStore)

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
        .handler(() => {
          const payments = database.tables.incomingPayment.selectAll()

          return createJsonResponse({
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
          }),
        )
        .handler((request) => {
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

          return createJsonResponse(formatIncomingPayment(payment))
        })
      sig.onCleanup(dispose)
    }
  })
