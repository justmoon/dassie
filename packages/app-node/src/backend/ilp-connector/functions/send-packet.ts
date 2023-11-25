import { Reactor } from "@dassie/lib-reactive"

import { AccountPath } from "../../accounting/types/account-paths"
import { IlpPacket, IlpType } from "../schemas/ilp-packet-codec"
import { BtpEndpointInfo, SendBtpPackets } from "../senders/send-btp-packets"
import {
  IldcpEndpointInfo,
  SendIldcpPackets,
} from "../senders/send-ildcp-packets"
import {
  IlpHttpEndpointInfo,
  SendIlpHttpPackets,
} from "../senders/send-ilp-http-packets"
import { PeerEndpointInfo, SendPeerPackets } from "../senders/send-peer-packets"
import {
  PluginEndpointInfo,
  SendPluginPackets,
} from "../senders/send-plugin-packets"
import { PreparedIlpPacketEvent } from "../topics/prepared-ilp-packet"
import { IlpAddress } from "../types/ilp-address"

export interface CommonEndpointInfo {
  readonly accountPath: AccountPath
  readonly ilpAddress: IlpAddress
}

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
  | PluginEndpointInfo
  | IlpHttpEndpointInfo

export type PacketSender<TType extends EndpointInfo["type"]> = {
  sendPrepare: (parameters: PreparedPacketParameters<TType>) => void
  sendResult: (parameters: ResolvedPacketParameters<TType>) => void
}

type AllPacketSenders = {
  [K in EndpointInfo["type"]]: PacketSender<K>
}

export const SendPacket = (reactor: Reactor) => {
  const senders: AllPacketSenders = {
    peer: reactor.use(SendPeerPackets),
    ildcp: reactor.use(SendIldcpPackets),
    btp: reactor.use(SendBtpPackets),
    plugin: reactor.use(SendPluginPackets),
    http: reactor.use(SendIlpHttpPackets),
  }

  return <TType extends EndpointInfo["type"]>(
    parameters:
      | PreparedPacketParameters<TType>
      | ResolvedPacketParameters<TType>,
  ) => {
    const sender = senders[
      parameters.destinationEndpointInfo.type
    ] as PacketSender<TType>

    if ("prepare" in parameters) {
      sender.sendResult(parameters)
    } else {
      sender.sendPrepare(parameters)
    }
  }
}
