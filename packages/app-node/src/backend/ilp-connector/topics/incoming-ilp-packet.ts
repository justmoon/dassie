import { createTopic } from "@dassie/lib-reactive"

import {
  type IlpFulfillPacket,
  type IlpPacket,
  type IlpPreparePacket,
  type IlpRejectPacket,
} from "../ilp-packet-codec"

type WithAttachedPrepare<T extends IlpPacket> = T extends
  | IlpFulfillPacket
  | IlpRejectPacket
  ? T & { prepare: IlpPreparePacket }
  : T

export type IlpPacketWithAttachedPrepare = WithAttachedPrepare<IlpPacket>

export interface IncomingIlpPacket {
  source: string
  packet: IlpPacketWithAttachedPrepare
  asUint8Array: Uint8Array
  requestId: number
}

export const incomingIlpPacketTopic = () => createTopic<IncomingIlpPacket>()
