import {
  Infer,
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

export const BtpContentType = {
  ApplicationOctetStream: 0,
  TextPlainUtf8: 1,
  ApplicationJson: 2,
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

export const btpErrorSchema = sequence({
  code: ia5String(3),
  name: ia5String(),
  triggeredAt: ia5String(),
  data: octetString([0, 8192]),
  protocolData: protocolDataSchema,
})

export const btpEnvelopeSchema = sequence({
  messageType: uint8Number(),
  requestId: uint32Number(),
  message: octetString(),
  // subprotocolData: protocolDataSchema,
})

export type ProtocolData = Infer<typeof protocolDataSchema>

export const generateBtpMessage = ({
  requestId,
  protocolData,
}: {
  requestId: number
  protocolData: ProtocolData
}) => {
  const serializationResult = btpMessageSchema.serialize({
    protocolData,
  })

  if (!serializationResult.success) {
    throw serializationResult.failure
  }

  const envelopeSerializationResult = btpEnvelopeSchema.serialize({
    messageType: BtpType.Message,
    requestId,
    message: serializationResult.value,
  })

  if (!envelopeSerializationResult.success) {
    throw envelopeSerializationResult.failure
  }

  return envelopeSerializationResult.value
}

export const generateBtpTransfer = ({
  requestId,
  amount,
  protocolData,
}: {
  requestId: number
  amount: bigint
  protocolData: ProtocolData
}) => {
  const serializationResult = btpTransferSchema.serialize({
    amount,
    protocolData,
  })

  if (!serializationResult.success) {
    throw serializationResult.failure
  }

  const envelopeSerializationResult = btpEnvelopeSchema.serialize({
    messageType: BtpType.Transfer,
    requestId,
    message: serializationResult.value,
  })

  if (!envelopeSerializationResult.success) {
    throw envelopeSerializationResult.failure
  }

  return envelopeSerializationResult.value
}

export const generateBtpResponse = ({
  requestId,
  protocolData,
}: {
  requestId: number
  protocolData: ProtocolData
}) => {
  const serializationResult = btpMessageSchema.serialize({
    protocolData,
  })

  if (!serializationResult.success) {
    throw serializationResult.failure
  }

  const envelopeSerializationResult = btpEnvelopeSchema.serialize({
    messageType: BtpType.Response,
    requestId,
    message: serializationResult.value,
  })

  if (!envelopeSerializationResult.success) {
    throw envelopeSerializationResult.failure
  }

  return envelopeSerializationResult.value
}

export const generateBtpError = ({
  requestId,
  code,
  name,
  triggeredAt,
  data,
  protocolData,
}: {
  requestId: number
  code: string
  name: string
  triggeredAt: string
  data: Uint8Array
  protocolData: ProtocolData
}) => {
  const serializationResult = btpErrorSchema.serialize({
    code,
    name,
    triggeredAt,
    data,
    protocolData,
  })

  if (!serializationResult.success) {
    throw serializationResult.failure
  }

  const envelopeSerializationResult = btpEnvelopeSchema.serialize({
    messageType: BtpType.Error,
    requestId,
    message: serializationResult.value,
  })

  if (!envelopeSerializationResult.success) {
    throw envelopeSerializationResult.failure
  }

  return envelopeSerializationResult.value
}
