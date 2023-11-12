import type { Simplify } from "type-fest"

import {
  type Infer,
  type InferSerialize,
  defineClass,
  defineObjectSet,
  ia5String,
  octetString,
  openType,
  sequence,
  uint8Number,
  uint64Bigint,
  utf8String,
} from "@dassie/lib-oer"
import { isFailure } from "@dassie/lib-type-utils"

export const IlpType = {
  Prepare: 12,
  Fulfill: 13,
  Reject: 14,
} as const

export type IlpType = (typeof IlpType)[keyof typeof IlpType]

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

const ilpPacketClass = defineClass({
  packetType: uint8Number(),
  data: openType,
})

const ilpPacketSet = defineObjectSet(ilpPacketClass, [
  {
    packetType: IlpType.Prepare,
    data: ilpPrepareSchema,
  },
  {
    packetType: IlpType.Fulfill,
    data: ilpFulfillSchema,
  },
  {
    packetType: IlpType.Reject,
    data: ilpRejectSchema,
  },
] as const)

export const ilpPacketSchema = sequence({
  type: ilpPacketSet.packetType,
  data: ilpPacketSet.data,
})

export type IlpPreparePacket = Simplify<Infer<typeof ilpPrepareSchema>>
export type IlpFulfillPacket = Simplify<Infer<typeof ilpFulfillSchema>>
export type IlpRejectPacket = Simplify<Infer<typeof ilpRejectSchema>>

export type IlpPacket =
  | ({ type: typeof IlpType.Prepare } & IlpPreparePacket)
  | ({ type: typeof IlpType.Fulfill } & IlpFulfillPacket)
  | ({ type: typeof IlpType.Reject } & IlpRejectPacket)

export const parseIlpPacket = (packet: Uint8Array): IlpPacket => {
  const parseResult = ilpPacketSchema.parse(packet)

  if (isFailure(parseResult)) {
    throw new Error("Failed to parse ILP packet", {
      cause: parseResult,
    })
  }

  const { type, data } = parseResult.value

  switch (type) {
    case IlpType.Prepare: {
      return { type, ...data }
    }
    case IlpType.Fulfill: {
      return { type, ...data }
    }
    case IlpType.Reject: {
      return { type, ...data }
    }

    default: {
      throw new Error("Unknown ILP packet type")
    }
  }
}

export const serializeIlpPacket = ({
  type,
  ...data
}: IlpPacket): Uint8Array => {
  const serializationResult = ilpPacketSchema.serialize({
    type,
    data,
  } as InferSerialize<typeof ilpPacketSchema>)

  if (isFailure(serializationResult)) {
    throw new Error("Failed to serialize ILP packet", {
      cause: serializationResult,
    })
  }

  return serializationResult
}
