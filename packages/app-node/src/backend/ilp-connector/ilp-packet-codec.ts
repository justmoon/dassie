import {
  ia5String,
  octetString,
  sequence,
  uint8Number,
  uint64Bigint,
  utf8String,
} from "@dassie/lib-oer"

export const IlpType = {
  Prepare: 12,
  Fulfill: 13,
  Reject: 14,
} as const

export type IlpType = typeof IlpType[keyof typeof IlpType]

export const ilpEnvelopeSchema = sequence({
  packetType: uint8Number(),
  data: octetString(),
})

export const ilpPrepareSchema = sequence({
  amount: uint64Bigint(),
  expiresAt: ia5String(17),
  executionCondition: octetString(32),
  destination: ia5String([0, 1023]),
  data: octetString([0, 32_767]),
})

export const ilpFulfillSchema = sequence({
  fulfillment: octetString(32),
  data: octetString([0, 32_767]),
})

export const ilpRejectSchema = sequence({
  code: ia5String(3),
  triggeredBy: ia5String(),
  message: utf8String([0, 8191]),
  data: octetString([0, 32_767]),
})
