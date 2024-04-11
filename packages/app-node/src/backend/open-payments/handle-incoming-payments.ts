import { nanoid } from "nanoid"
import { z } from "zod"

import { cors, createJsonResponse, parseBodyZod } from "@dassie/lib-http-server"
import { createActor } from "@dassie/lib-reactive"

import { DatabaseConfigStore } from "../config/database-config"
import { Database } from "../database/open-database"
import { HttpsRouter } from "../http-server/serve-https"
import { StreamServerServiceActor } from "../spsp-server/stream-server"
import { PAYMENT_POINTER_ROOT } from "./constants/payment-pointer"
import { createIncomingPaymentFormatter } from "./utils/format-incoming-payment"

export const HandleIncomingPaymentsActor = () =>
  createActor((sig) => {
    const http = sig.reactor.use(HttpsRouter)
    const database = sig.reactor.use(Database)
    const streamServer = sig.readAndTrack(StreamServerServiceActor)
    const { url } = sig.readAndTrack(DatabaseConfigStore)

    if (!streamServer) return

    const formatIncomingPayment = createIncomingPaymentFormatter({
      url,
      streamServer,
    })

    // List incoming payments
    http
      .get()
      .path(`${PAYMENT_POINTER_ROOT}/incoming-payments`)
      .use(cors)
      .handler(sig, () => {
        const payments = database.tables.incomingPayment.selectAll()

        return createJsonResponse({
          result: payments.map((payment) => formatIncomingPayment(payment)),
        })
      })

    // Create a new incoming payment
    http
      .post()
      .path(`${PAYMENT_POINTER_ROOT}/incoming-payments`)
      .use(cors)
      .use(
        parseBodyZod(
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
        ),
      )
      .handler(sig, (request) => {
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
  })
