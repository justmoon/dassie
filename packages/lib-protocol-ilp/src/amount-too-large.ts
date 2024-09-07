import { type Infer, sequence, uint64Bigint } from "@dassie/lib-oer"
import { isFailure } from "@dassie/lib-type-utils"

export const amountTooLargeDataSchema = sequence({
  receivedAmount: uint64Bigint(),
  maximumAmount: uint64Bigint(),
})

export type AmountTooLargeData = Infer<typeof amountTooLargeDataSchema>

export function serializeAmountTooLargeData({
  receivedAmount,
  maximumAmount,
}: AmountTooLargeData) {
  return amountTooLargeDataSchema.serializeOrThrow({
    receivedAmount,
    maximumAmount,
  })
}

export function parseAmountTooLargeData(data: Uint8Array) {
  const result = amountTooLargeDataSchema.parse(data)

  if (isFailure(result)) {
    return result
  }

  return result.value
}
