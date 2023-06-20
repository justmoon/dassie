import { createTopic } from "@dassie/lib-reactive"

import { IlpPacket, IlpPreparePacket } from "../schemas/ilp-packet-codec"
import { PreparedIlpPacketEvent } from "./prepared-ilp-packet"

export interface ResolvedIlpPacketEvent {
  prepare: PreparedIlpPacketEvent
  serializedPacket: Uint8Array
  parsedPacket: Exclude<IlpPacket, IlpPreparePacket>
}

export const resolvedIlpPacketTopic = () =>
  createTopic<ResolvedIlpPacketEvent>()
