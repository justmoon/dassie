import type { Simplify } from "type-fest"

import {
  type Infer,
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

export const ilpAddress = ia5String([1, 1023])

export const ilpPrepareSchema = sequence({
  amount: uint64Bigint(),
  expiresAt: ia5String(17),
  executionCondition: octetString(32),
  destination: ilpAddress,
  data: octetString([0, 32_767]),
})

export const ilpFulfillSchema = sequence({
  fulfillment: octetString(32),
  data: octetString([0, 32_767]),
})

export const ilpRejectSchema = sequence({
  code: ia5String(3),
  triggeredBy: ilpAddress,
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

export type IlpPacket = Simplify<Infer<typeof ilpPacketSchema>>

export const parseIlpPacket = (packet: Uint8Array): IlpPacket => {
  const parseResult = ilpPacketSchema.parse(packet)

  if (isFailure(parseResult)) {
    throw new Error("Failed to parse ILP packet", {
      cause: parseResult,
    })
  }

  return parseResult.value
}

export const serializeIlpPacket = (packet: IlpPacket): Uint8Array => {
  const serializationResult = ilpPacketSchema.serialize(packet)

  if (isFailure(serializationResult)) {
    throw new Error("Failed to serialize ILP packet", {
      cause: serializationResult,
    })
  }

  return serializationResult
}
