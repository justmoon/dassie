import { createConnection } from "ilp-protocol-stream"
import { z } from "zod"

import { createScope } from "@dassie/lib-reactive"
import { isObject, tell } from "@dassie/lib-type-utils"

import { OwnerLedgerIdSignal } from "../../accounting/signals/owner-ledger-id"
import type { DassieReactor } from "../../base/types/dassie-base"
import { Database } from "../../database/open-database"
import { payment as logger } from "../../logger/instances"
import { paymentPointerToUrl } from "../../utils/resolve-payment-pointer"
import { walletSchema } from "../schemas/wallet"
import { CreatePlugin } from "./create-plugin"

interface MakePaymentParameters {
  /**
   * Unique identifier for this payment.
   */
  id: string

  /**
   * The payment pointer to send the payment to.
   */
  destination: string

  /**
   * The amount to send in the destination currency.
   */
  amount: bigint
}

const incomingPaymentSchema = z.object({
  id: z.string(),
  walletAddress: z.string(),
  completed: z.boolean(),
  incomingAmount: z
    .object({
      value: z.string(),
      assetCode: z.string(),
      assetScale: z.number().int().min(0).max(255),
    })
    .optional(),
  receivedAmount: z.object({
    value: z.string(),
    assetCode: z.string(),
    assetScale: z.number().int().min(0).max(255),
  }),
  expiresAt: z.string().optional(),
  metadata: z.object({}).passthrough().optional(),
  createdAt: z.string(),
  updatedAt: z.string(),
  methods: z.preprocess(
    // Preprocess to remove any methods that look valid but are not ILP
    //
    // Invalid methods (for example those that do not have a type) intentionally
    // pass through so they trigger a validation error.
    (methods) =>
      Array.isArray(methods) ?
        methods.filter(
          (element) =>
            !isObject(element) ||
            !("type" in element) ||
            typeof element["type"] !== "string" ||
            element["type"] === "ilp",
        )
      : methods,
    z
      .array(
        z.object({
          type: z.literal("ilp"),
          ilpAddress: z.string().max(1023),
          sharedSecret: z.string(),
        }),
      )
      .nonempty(),
  ),
})

export const MakePayment = (reactor: DassieReactor) => {
  const database = reactor.use(Database)
  const ownerLedgerIdSignal = reactor.use(OwnerLedgerIdSignal)
  const createPlugin = reactor.use(CreatePlugin)

  return async function makePayment({
    id,
    destination,
    amount,
  }: MakePaymentParameters) {
    // Step 1: Resolve the payment pointer
    const url = paymentPointerToUrl(destination)

    const walletInfoResult = await fetch(url, {
      method: "GET",
      headers: {
        Accept: "application/json",
      },
    })

    if (!walletInfoResult.ok) {
      throw new Error(`Failed to fetch payment pointer ${destination}`)
    }

    const walletInfoRaw = await walletInfoResult.json()

    const walletInfo = walletSchema.parse(walletInfoRaw)

    // Step 2: Create the incoming payment at the destination

    const incomingPaymentResult = await fetch(
      `${walletInfo.resourceServer}/incoming-payments`,
      {
        method: "POST",
        headers: {
          Accept: "application/json",
          "Content-Type": "application/json",
        },
        body: JSON.stringify({
          walletAddress: walletInfo.id,
          incomingAmount: {
            value: amount.toString(),
            assetCode: walletInfo.assetCode,
            assetScale: walletInfo.assetScale,
          },
        }),
      },
    )

    if (!incomingPaymentResult.ok) {
      throw new Error(
        `Failed to create incoming payment for payment pointer ${destination}`,
      )
    }

    const incomingPaymentRaw = await incomingPaymentResult.json()

    const incomingPayment = incomingPaymentSchema.parse(incomingPaymentRaw)

    const method = incomingPayment.methods[0]

    // Step 3: Quoting an amount

    // TODO: Implement; for now, we just assume 1:1
    const sourceAmount = amount

    // Step 4: Create the outgoing payment locally
    database.tables.outgoingPayment.insertOne({
      id,
      destination: incomingPayment.id,
      ledger: ownerLedgerIdSignal.read(),
      total_amount: BigInt(sourceAmount),
      metadata: "{}",
    })

    // TODO: Make sure this is cleaned up even in the case of failure
    const scope = createScope("payment")
    const plugin = createPlugin(scope)

    const connection = await createConnection({
      plugin,
      destinationAccount: method.ilpAddress,
      sharedSecret: Buffer.from(method.sharedSecret, "base64"),
    })

    logger.debug?.("created STREAM connection", {
      id,
    })

    const stream = connection.createStream()

    stream.setSendMax(String(sourceAmount))

    let sentAmount = 0n

    stream.on("outgoing_money", (amountString: string) => {
      const amount = BigInt(amountString)

      sentAmount += amount

      logger.debug?.("sent money", {
        id,
        newlySentAmount: amount,
        sentAmount,
        totalAmount: sourceAmount,
      })

      if (sentAmount + amount >= sourceAmount) {
        logger.debug?.("payment complete", { id })
        connection
          .end()
          .catch((error: unknown) => {
            logger.error("error ending connection", {
              id,
              error,
            })
          })
          .finally(() => {
            tell(() => scope.dispose())
          })
      }
    })
  }
}
