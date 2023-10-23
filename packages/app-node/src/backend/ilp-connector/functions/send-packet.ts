import { Promisable } from "type-fest"

import { Actor, ActorApiHandler, ActorContext } from "@dassie/lib-reactive"

import {
  BtpEndpointInfo,
  SendBtpPacketsActor,
} from "../senders/send-btp-packets"
import {
  IldcpEndpointInfo,
  SendIldcpPacketsActor,
} from "../senders/send-ildcp-packets"
import {
  PeerEndpointInfo,
  SendPeerPacketsActor,
} from "../senders/send-peer-packets"
import {
  PluginEndpointInfo,
  SendPluginPacketsActor,
} from "../senders/send-plugin-packets"
import { PreparedIlpPacketEvent } from "../topics/prepared-ilp-packet"
import { ResolvedIlpPacketEvent } from "../topics/resolved-ilp-packet"

export interface CommonEndpointInfo {
  readonly accountPath: string
  readonly ilpAddress: string
}

export type EndpointInfo =
  | PeerEndpointInfo
  | IldcpEndpointInfo
  | BtpEndpointInfo
  | PluginEndpointInfo

export type PreparedPacketParameters<TType extends EndpointInfo["type"]> =
  EndpointInfo & { type: TType } & PreparedIlpPacketEvent

export type ResolvedPacketParameters<TType extends EndpointInfo["type"]> =
  EndpointInfo & { type: TType } & ResolvedIlpPacketEvent

export type PacketSender<TType extends EndpointInfo["type"]> = Actor<{
  sendPrepare: ActorApiHandler<
    (parameters: PreparedPacketParameters<TType>) => Promisable<void>
  >
  sendResult: ActorApiHandler<
    (parameters: ResolvedPacketParameters<TType>) => Promisable<void>
  >
}>

type AllPacketSenders = {
  [K in EndpointInfo["type"]]: PacketSender<K>
}

export const createPacketSender = (sig: ActorContext) => {
  const senders: AllPacketSenders = {
    peer: sig.use(SendPeerPacketsActor),
    ildcp: sig.use(SendIldcpPacketsActor),
    btp: sig.use(SendBtpPacketsActor),
    plugin: sig.use(SendPluginPacketsActor),
  }

  for (const sender of Object.values(senders)) {
    sender.run(sig.reactor, sig)
  }

  return <TType extends EndpointInfo["type"]>(
    client: EndpointInfo & { type: TType },
    event: PreparedIlpPacketEvent | ResolvedIlpPacketEvent,
  ) => {
    const sender = senders[client.type] as PacketSender<TType>

    if ("prepare" in event) {
      sender.api.sendResult.tell({
        ...client,
        ...event,
      })
    } else {
      sender.api.sendPrepare.tell({
        ...client,
        ...event,
      })
    }
  }
}
