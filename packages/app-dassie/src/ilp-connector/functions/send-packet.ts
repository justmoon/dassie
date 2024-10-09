import { type IlpPacket, IlpType } from "@dassie/lib-protocol-ilp"

import type { DassieReactor } from "../../base/types/dassie-base"
import {
  type BtpEndpointInfo,
  SendBtpPackets,
} from "../senders/send-btp-packets"
import {
  type IldcpEndpointInfo,
  SendIldcpPackets,
} from "../senders/send-ildcp-packets"
import {
  type IlpHttpEndpointInfo,
  SendIlpHttpPackets,
} from "../senders/send-ilp-http-packets"
import {
  type LocalEndpointInfo,
  SendLocalPackets,
} from "../senders/send-local-packets"
import {
  type PeerEndpointInfo,
  SendPeerPackets,
} from "../senders/send-peer-packets"
import type { PreparedIlpPacketEvent } from "../topics/prepared-ilp-packet"

export interface CommonEndpointInfo {}

export interface PreparedPacketParameters<
  TType extends EndpointInfo["type"] = EndpointInfo["type"],
> {
  readonly serializedPacket: Uint8Array
  readonly parsedPacket: Extract<IlpPacket, { type: typeof IlpType.Prepare }>
  readonly outgoingRequestId: number
  readonly sourceEndpointInfo: EndpointInfo
  readonly destinationEndpointInfo: EndpointInfo & { type: TType }
}

export interface ResolvedPacketParameters<
  TType extends EndpointInfo["type"] = EndpointInfo["type"],
> {
  readonly prepare: PreparedIlpPacketEvent
  readonly serializedPacket: Uint8Array
  readonly parsedPacket: Exclude<IlpPacket, { type: typeof IlpType.Prepare }>
  readonly destinationEndpointInfo: EndpointInfo & { type: TType }
}

export type EndpointInfo =
  | PeerEndpointInfo
  | IldcpEndpointInfo
  | BtpEndpointInfo
  | LocalEndpointInfo
  | IlpHttpEndpointInfo

export type PacketSender<TType extends EndpointInfo["type"]> = {
  sendPrepare: (parameters: PreparedPacketParameters<TType>) => void
  sendResult: (parameters: ResolvedPacketParameters<TType>) => void
}

type AllPacketSenders = {
  [K in EndpointInfo["type"]]: PacketSender<K>
}

export const SendPacket = (reactor: DassieReactor) => {
  const senders: AllPacketSenders = {
    peer: reactor.use(SendPeerPackets),
    ildcp: reactor.use(SendIldcpPackets),
    btp: reactor.use(SendBtpPackets),
    local: reactor.use(SendLocalPackets),
    http: reactor.use(SendIlpHttpPackets),
  }

  function sendPacket<TType extends EndpointInfo["type"]>(
    parameters:
      | PreparedPacketParameters<TType>
      | ResolvedPacketParameters<TType>,
  ) {
    const sender = senders[
      parameters.destinationEndpointInfo.type
    ] as PacketSender<TType>

    if ("prepare" in parameters) {
      sender.sendResult(parameters)
    } else {
      sender.sendPrepare(parameters)
    }
  }

  return sendPacket
}
