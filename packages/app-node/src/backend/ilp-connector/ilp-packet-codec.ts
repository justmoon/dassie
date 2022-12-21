import {
  Infer,
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

export type IlpPreparePacket = Infer<typeof ilpPrepareSchema>
export type IlpFulfillPacket = Infer<typeof ilpFulfillSchema>
export type IlpRejectPacket = Infer<typeof ilpRejectSchema>

export type IlpPacket =
  | ({ type: typeof IlpType.Prepare } & IlpPreparePacket)
  | ({ type: typeof IlpType.Fulfill } & IlpFulfillPacket)
  | ({ type: typeof IlpType.Reject } & IlpRejectPacket)

export const parseIlpPacket = (packet: Uint8Array): IlpPacket => {
  const envelopeParseResult = ilpEnvelopeSchema.parse(packet)

  if (!envelopeParseResult.success) {
    throw new Error("Failed to parse ILP envelope", {
      cause: envelopeParseResult.error,
    })
  }

  switch (envelopeParseResult.value.packetType) {
    case IlpType.Prepare: {
      const prepareParseResult = ilpPrepareSchema.parse(
        envelopeParseResult.value.data
      )

      if (!prepareParseResult.success) {
        throw new Error("Failed to parse ILP prepare", {
          cause: prepareParseResult.error,
        })
      }

      return {
        type: IlpType.Prepare,
        ...prepareParseResult.value,
      }
    }

    case IlpType.Fulfill: {
      const fulfillParseResult = ilpFulfillSchema.parse(
        envelopeParseResult.value.data
      )

      if (!fulfillParseResult.success) {
        throw new Error("Failed to parse ILP fulfill", {
          cause: fulfillParseResult.error,
        })
      }

      return {
        type: IlpType.Fulfill,
        ...fulfillParseResult.value,
      }
    }

    case IlpType.Reject: {
      const rejectParseResult = ilpRejectSchema.parse(
        envelopeParseResult.value.data
      )

      if (!rejectParseResult.success) {
        throw new Error("Failed to parse ILP reject", {
          cause: rejectParseResult.error,
        })
      }

      return {
        type: IlpType.Reject,
        ...rejectParseResult.value,
      }
    }
    default: {
      throw new Error("Unknown ILP packet type")
    }
  }
}

export const serializeIlpPacket = (packet: IlpPacket): Uint8Array => {
  let packetSerializationResult: ReturnType<typeof ilpPrepareSchema.serialize>
  switch (packet.type) {
    case IlpType.Prepare: {
      packetSerializationResult = ilpPrepareSchema.serialize(packet)
      break
    }

    case IlpType.Fulfill: {
      packetSerializationResult = ilpFulfillSchema.serialize(packet)
      break
    }

    case IlpType.Reject: {
      packetSerializationResult = ilpRejectSchema.serialize(packet)
      break
    }

    default: {
      throw new Error("Unknown ILP packet type")
    }
  }

  if (!packetSerializationResult.success) {
    throw new Error("Failed to serialize ILP packet", {
      cause: packetSerializationResult.error,
    })
  }

  const envelope = ilpEnvelopeSchema.serialize({
    packetType: packet.type,
    data: packetSerializationResult.value,
  })

  if (!envelope.success) {
    throw new Error("Failed to serialize ILP envelope", {
      cause: envelope.error,
    })
  }

  return envelope.value
}
