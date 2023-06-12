import { z } from "zod"

export const spspResultSchema = z.object({
  destination_account: z.string(),
  shared_secret: z.string(),
})

export const resolvePaymentPointer = async (paymentPointer: string) => {
  const spspUrl = new URL(paymentPointer.replace(/^\$/, "https://"))

  if (spspUrl.pathname === "/") {
    spspUrl.pathname = "/.well-known/pay"
  }

  const spspFetchResult = await fetch(spspUrl)
  const spspRawResult = (await spspFetchResult.json()) as unknown

  const spspResponse = spspResultSchema.parse(spspRawResult)

  return spspResponse
}
