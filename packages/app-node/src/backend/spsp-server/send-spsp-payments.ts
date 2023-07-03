import { createConnection } from "ilp-protocol-stream"

import { createLogger } from "@dassie/lib-logger"
import { createActor, createStore } from "@dassie/lib-reactive"

import { resolvePaymentPointer } from "../utils/resolve-payment-pointer"
import { managePlugins } from "./manage-plugins"

const logger = createLogger("das:node:send-spsp-payments")

export interface OutgoingSpspPayment {
  id: string
  destination: string
  totalAmount: bigint
  sentAmount: bigint
}

export const spspPaymentQueueStore = () =>
  createStore([] as OutgoingSpspPayment[], {
    addPayment: (payment: OutgoingSpspPayment) => (state) =>
      [...state, payment],
    // eslint-disable-next-line unicorn/consistent-function-scoping
    shift: () => (state) => state.slice(1),
  })

export const sendSpspPayments = () =>
  createActor(async (sig) => {
    const pluginManager = sig.use(managePlugins)
    const nextPayment = sig.get(spspPaymentQueueStore, (queue) => queue[0])

    if (nextPayment) {
      logger.debug("initiating payment", {
        id: nextPayment.id,
        to: nextPayment.destination,
        totalAmount: nextPayment.totalAmount,
      })

      const {
        destination_account: destinationAccount,
        shared_secret: sharedSecret,
      } = await resolvePaymentPointer(nextPayment.destination)

      logger.debug("resolved payment pointer", {
        id: nextPayment.id,
        destinationAccount,
      })

      const plugin = await pluginManager.ask("createPlugin", undefined)

      const connection = await createConnection({
        plugin,
        destinationAccount,
        sharedSecret: Buffer.from(sharedSecret, "base64"),
      })

      logger.debug("created STREAM connection", {
        id: nextPayment.id,
      })

      const stream = connection.createStream()

      stream.setSendMax(
        String(nextPayment.totalAmount - nextPayment.sentAmount)
      )

      stream.on("outgoing_money", (amountString: string) => {
        const amount = BigInt(amountString)

        // TODO: Mutating this object doesn't seem like the best way to handle this?
        nextPayment.sentAmount += amount

        logger.debug("sent money", {
          id: nextPayment.id,
          newlySentAmount: amount,
          sentAmount: nextPayment.sentAmount,
          totalAmount: nextPayment.totalAmount,
        })

        if (nextPayment.sentAmount + amount >= nextPayment.totalAmount) {
          logger.debug("payment complete", { id: nextPayment.id })
          connection.end().catch((error: unknown) => {
            logger.error("error ending connection", {
              id: nextPayment.id,
              error,
            })
          })
          sig.use(spspPaymentQueueStore).shift()
        }
      })
    }
  })
