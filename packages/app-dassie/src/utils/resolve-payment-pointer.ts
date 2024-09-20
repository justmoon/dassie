import { z } from "zod"

import { walletSchema } from "../open-payments/schemas/wallet"

export const spspResultSchema = z.object({
  destination_account: z.string(),
  shared_secret: z.string(),
})

export const paymentPointerToUrl = (paymentPointer: string) => {
  const url = new URL(paymentPointer.replace(/^\$/, "https://"))

  if (url.pathname === "/") {
    url.pathname = "/.well-known/pay"
  }

  return url
}

export const resolvePaymentPointer = async (paymentPointer: string) => {
  const spspUrl = paymentPointerToUrl(paymentPointer)

  const walletFetchResult = await fetch(spspUrl)
  const walletRawResult = await walletFetchResult.json()

  const wallet = walletSchema.parse(walletRawResult)

  return wallet
}
