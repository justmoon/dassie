import {
  ia5String,
  octetString,
  sequence,
  sequenceOf,
  uint8Number,
  uint32Number,
  uint64Bigint,
} from "@dassie/lib-oer"

export const BtpType = {
  Response: 1,
  Error: 2,
  Message: 6,
  Transfer: 7,
} as const

export type BtpType = typeof BtpType[keyof typeof BtpType]

export const protocolDataSchema = sequenceOf(
  sequence({
    protocolName: ia5String(),
    contentType: uint8Number(),
    data: octetString(),
  })
)

export const btpMessageSchema = sequence({
  protocolData: protocolDataSchema,
})

export const btpTransferSchema = sequence({
  amount: uint64Bigint(),
  protocolData: protocolDataSchema,
})

export const btpEnvelopeSchema = sequence({
  messageType: uint8Number(),
  requestId: uint32Number(),
  message: octetString(),
  // subprotocolData: protocolDataSchema,
})
