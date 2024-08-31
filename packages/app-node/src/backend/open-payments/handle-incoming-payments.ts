import { nanoid } from "nanoid"
import { z } from "zod"

import { cors, createJsonResponse, parseBodyZod } from "@dassie/lib-http-server"
import { createActor } from "@dassie/lib-reactive"

import { OwnerLedgerIdSignal } from "../accounting/signals/owner-ledger-id"
import { DatabaseConfigStore } from "../config/database-config"
import { Database } from "../database/open-database"
import { HttpsRouter } from "../http-server/values/https-router"
import { StreamServerServiceActor } from "./stream-server"
import { createIncomingPaymentFormatter } from "./utils/format-incoming-payment"

export const HandleIncomingPaymentsActor = () =>
  createActor((sig) => {
    const http = sig.reactor.use(HttpsRouter)
    const database = sig.reactor.use(Database)
    const streamServer = sig.readAndTrack(StreamServerServiceActor)
    const { url } = sig.readAndTrack(DatabaseConfigStore)
    const ownerLedgerIdSignal = sig.reactor.use(OwnerLedgerIdSignal)

    if (!streamServer) return

    const formatIncomingPayment = createIncomingPaymentFormatter({
      url,
      streamServer,
    })

    // List incoming payments
    http
      .get()
      .path(`/incoming-payments`)
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
      .path(`/incoming-payments`)
      .use(cors)
      .use(
        parseBodyZod(
          z.object({
            walletAddress: z.string(),
            incomingAmount: z.object({
              value: z.string(),
              assetCode: z.string(),
              assetScale: z.number().int().gte(0).lte(255),
            }),
            expiresAt: z.string().optional(),
            metadata: z.object({}).passthrough().optional(),
          }),
        ),
      )
      .handler(sig, (request) => {
        const { incomingAmount, metadata } = request.body
        const id = nanoid()
        const payment = {
          id,
          ledger: ownerLedgerIdSignal.read(),
          total_amount: BigInt(incomingAmount.value),
          metadata: JSON.stringify(metadata ?? {}),
        }

        database.tables.incomingPayment.insertOne(payment)

        return createJsonResponse(formatIncomingPayment(payment))
      })
  })
