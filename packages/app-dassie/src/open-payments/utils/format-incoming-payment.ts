import type { Server as StreamServer } from "ilp-protocol-stream"

import { PAYMENT_POINTER_PATH } from "../constants/payment-pointer"
import type { IncomingPaymentRow } from "../tables/incoming-payment"

export interface IncomingPaymentFormatterOptions {
  url: string
  streamServer: StreamServer
}

export const createIncomingPaymentFormatter =
  ({ url, streamServer }: IncomingPaymentFormatterOptions) =>
  (payment: IncomingPaymentRow) => {
    const createdAt = new Date().toISOString()

    const { destinationAccount, sharedSecret } =
      streamServer.generateAddressAndSecret({
        connectionTag: payment.id,
      })

    // TODO: Received amount should not be hardcoded, but should be calculated
    // from the ledger

    return {
      id: `${url}/incoming-payments/${payment.id}`,
      walletAddress: `${url}${PAYMENT_POINTER_PATH}`,
      completed: false,
      incomingAmount: {
        value: payment.total_amount.toString(),
        assetCode: "USD",
        assetScale: 2,
      },
      receivedAmount: {
        value: "0",
        assetCode: "USD",
        assetScale: 2,
      },
      methods: [
        {
          type: "ilp",
          ilpAddress: destinationAccount,
          sharedSecret: sharedSecret.toString("base64url"),
        },
      ],
      createdAt,
      updatedAt: createdAt,
    }
  }
