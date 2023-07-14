import type { Server as StreamServer } from "ilp-protocol-stream"

import { PAYMENT_POINTER_ROOT } from "../constants/payment-pointer"
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

    return {
      id: `${url}${PAYMENT_POINTER_ROOT}/incoming-payments/${payment.id}`,
      incomingAmount: {
        value: payment.total_amount.toString(),
        assetCode: "USD",
        assetScale: 2,
      },
      receivedAmount: {
        value: payment.received_amount.toString(),
        assetCode: "USD",
        assetScale: 2,
      },
      ilpStreamConnection: {
        id: `${url}${PAYMENT_POINTER_ROOT}/connections/${payment.id}`,
        ilpAddress: destinationAccount,
        sharedSecret: sharedSecret.toString("base64url"),
        assetCode: "USD",
        assetScale: 2,
      },
      createdAt,
      updatedAt: createdAt,
    }
  }
