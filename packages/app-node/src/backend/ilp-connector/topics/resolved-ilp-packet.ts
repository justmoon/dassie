import { createTopic } from "@dassie/lib-reactive"

import { EndpointInfo } from "../functions/send-packet"
import { IlpPacket, IlpType } from "../schemas/ilp-packet-codec"
import { PreparedIlpPacketEvent } from "./prepared-ilp-packet"

export interface ResolvedIlpPacketEvent {
  readonly prepare: PreparedIlpPacketEvent
  readonly serializedPacket: Uint8Array
  readonly parsedPacket: Exclude<IlpPacket, { type: typeof IlpType.Prepare }>
  readonly destinationEndpointInfo: EndpointInfo
}

export const ResolvedIlpPacketTopic = () =>
  createTopic<ResolvedIlpPacketEvent>()
