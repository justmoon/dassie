import { Reactor } from "@dassie/lib-reactive"

import { AccountPath } from "../../accounting/types/accounts"
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
import { ResolvedIlpPacketEvent } from "../topics/resolved-ilp-packet"

export interface CommonEndpointInfo {
  readonly accountPath: AccountPath
  readonly ilpAddress: string
}

export type EndpointInfo =
  | PeerEndpointInfo
  | IldcpEndpointInfo
  | BtpEndpointInfo
  | PluginEndpointInfo
  | IlpHttpEndpointInfo

export type PreparedPacketParameters<TType extends EndpointInfo["type"]> =
  EndpointInfo & { type: TType } & PreparedIlpPacketEvent

export type ResolvedPacketParameters<TType extends EndpointInfo["type"]> =
  EndpointInfo & { type: TType } & ResolvedIlpPacketEvent

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
    client: EndpointInfo & { type: TType },
    event: PreparedIlpPacketEvent | ResolvedIlpPacketEvent,
  ) => {
    const sender = senders[client.type] as PacketSender<TType>

    if ("prepare" in event) {
      sender.sendResult({
        ...client,
        ...event,
      })
    } else {
      sender.sendPrepare({
        ...client,
        ...event,
      })
    }
  }
}
