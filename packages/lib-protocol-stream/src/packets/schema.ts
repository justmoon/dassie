import {
  type Infer,
  defineClass,
  defineObjectSet,
  integerAsBigint,
  octetString,
  openType,
  sequence,
  sequenceOf,
  uint8Number,
  utf8String,
} from "@dassie/lib-oer"

import { ilpAddressSchema } from "@dassie/lib-protocol-ilp"

export const FrameType = {
  ConnectionClose: 0x01,
  ConnectionNewAddress: 0x02,
  ConnectionMaxData: 0x03,
  ConnectionDataBlocked: 0x04,
  ConnectionMaxStreamId: 0x05,
  ConnectionStreamIdBlocked: 0x06,
  ConnectionAssetDetails: 0x07,
  StreamClose: 0x10,
  StreamMoney: 0x11,
  StreamMaxMoney: 0x12,
  StreamMoneyBlocked: 0x13,
  StreamData: 0x14,
  StreamMaxData: 0x15,
  StreamDataBlocked: 0x16,
  StreamReceipt: 0x17,
} as const

export type FrameType = (typeof FrameType)[keyof typeof FrameType]

export const ErrorCode = {
  NoError: 0x01,
  InternalError: 0x02,
  EndpointBusy: 0x03,
  FlowControlError: 0x04,
  StreamIdError: 0x05,
  StreamStateError: 0x06,
  FrameFormatError: 0x07,
  ProtocolViolation: 0x08,
  ApplicationError: 0x09,
} as const

export type ErrorCode = (typeof ErrorCode)[keyof typeof ErrorCode]

export const frameConnectionCloseSchema = sequence({
  errorCode: uint8Number(),
  errorMessage: utf8String(),
})

export const frameConnectionNewAddressSchema = sequence({
  sourceAccount: ilpAddressSchema,
})

export const frameConnectionMaxDataSchema = sequence({
  maxOffset: integerAsBigint([0n, undefined]),
})

export const frameConnectionDataBlockedSchema = sequence({
  maxOffset: integerAsBigint([0n, undefined]),
})

export const frameConnectionMaxStreamIdSchema = sequence({
  maxStreamId: integerAsBigint([0n, undefined]),
})

export const frameConnectionStreamIdBlockedSchema = sequence({
  maxStreamId: integerAsBigint([0n, undefined]),
})

export const frameConnectionAssetDetailsSchema = sequence({
  sourceAssetCode: utf8String(),
  sourceAssetScale: uint8Number(),
})

export const frameStreamCloseSchema = sequence({
  streamId: integerAsBigint([0n, undefined]),
  errorCode: uint8Number(),
  errorMessage: utf8String(),
})

export const frameStreamMoneySchema = sequence({
  streamId: integerAsBigint([0n, undefined]),
  shares: integerAsBigint([0n, undefined]),
})

export const frameStreamMaxMoney = sequence({
  streamId: integerAsBigint([0n, undefined]),
  receiveMax: integerAsBigint([0n, undefined]),
  totalReceived: integerAsBigint([0n, undefined]),
})

export const frameStreamMoneyBlockedSchema = sequence({
  streamId: integerAsBigint([0n, undefined]),
  sendMax: integerAsBigint([0n, undefined]),
  totalSent: integerAsBigint([0n, undefined]),
})

export const frameStreamDataSchema = sequence({
  streamId: integerAsBigint([0n, undefined]),
  offset: integerAsBigint([0n, undefined]),
  data: octetString(),
})

export const frameStreamMaxDataSchema = sequence({
  streamId: integerAsBigint([0n, undefined]),
  maxOffset: integerAsBigint([0n, undefined]),
})

export const frameStreamDataBlockedSchema = sequence({
  streamId: integerAsBigint([0n, undefined]),
  maxOffset: integerAsBigint([0n, undefined]),
})

export const frameStreamReceiptSchema = sequence({
  streamId: integerAsBigint([0n, undefined]),
  receipt: octetString(),
})

const streamFrameClassDefinition = {
  type: uint8Number(),
  data: openType,
} as const

const streamFrameObjectSetDefinition = [
  {
    type: FrameType.ConnectionClose,
    data: frameConnectionCloseSchema,
  },
  {
    type: FrameType.ConnectionNewAddress,
    data: frameConnectionNewAddressSchema,
  },
  {
    type: FrameType.ConnectionMaxData,
    data: frameConnectionMaxDataSchema,
  },
  {
    type: FrameType.ConnectionDataBlocked,
    data: frameConnectionDataBlockedSchema,
  },
  {
    type: FrameType.ConnectionMaxStreamId,
    data: frameConnectionMaxStreamIdSchema,
  },
  {
    type: FrameType.ConnectionStreamIdBlocked,
    data: frameConnectionStreamIdBlockedSchema,
  },
  {
    type: FrameType.ConnectionAssetDetails,
    data: frameConnectionAssetDetailsSchema,
  },
  {
    type: FrameType.StreamClose,
    data: frameStreamCloseSchema,
  },
  {
    type: FrameType.StreamMoney,
    data: frameStreamMoneySchema,
  },
  {
    type: FrameType.StreamMaxMoney,
    data: frameStreamMaxMoney,
  },
  {
    type: FrameType.StreamMoneyBlocked,
    data: frameStreamMoneyBlockedSchema,
  },
  {
    type: FrameType.StreamData,
    data: frameStreamDataSchema,
  },
  {
    type: FrameType.StreamMaxData,
    data: frameStreamMaxDataSchema,
  },
  {
    type: FrameType.StreamDataBlocked,
    data: frameStreamDataBlockedSchema,
  },
  {
    type: FrameType.StreamReceipt,
    data: frameStreamReceiptSchema,
  },
] as const

const streamFrameClass = defineClass(streamFrameClassDefinition)
const streamFrameObjectSet = defineObjectSet(
  streamFrameClass,
  streamFrameObjectSetDefinition,
)

export const streamFrameSchema = sequence({
  type: streamFrameObjectSet.type,
  data: streamFrameObjectSet.data,
})

export type StreamFrame = Infer<typeof streamFrameSchema>

export const streamPacketSchema = sequence({
  version: uint8Number().constant(1),
  packetType: uint8Number(),
  sequence: integerAsBigint([0n, undefined]),
  amount: integerAsBigint([0n, undefined]),
  frames: sequenceOf(streamFrameSchema),
})

export type Packet = Infer<typeof streamPacketSchema>
