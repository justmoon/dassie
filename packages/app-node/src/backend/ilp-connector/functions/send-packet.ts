import { Promisable } from "type-fest"

import { Actor, ActorContext } from "@dassie/lib-reactive"

import { BtpDestinationInfo, sendBtpPackets } from "../senders/send-btp-packets"
import {
  IldcpDestinationInfo,
  sendIldcpPackets,
} from "../senders/send-ildcp-packets"
import {
  PeerDestinationInfo,
  sendPeerPackets,
} from "../senders/send-peer-packets"
import {
  PluginDestinationInfo,
  sendPluginPackets,
} from "../senders/send-plugin-packets"
import { PreparedIlpPacketEvent } from "../topics/prepared-ilp-packet"
import { ResolvedIlpPacketEvent } from "../topics/resolved-ilp-packet"

export type IlpDestinationInfo =
  | PeerDestinationInfo
  | IldcpDestinationInfo
  | BtpDestinationInfo
  | PluginDestinationInfo

export type PreparedPacketParameters<TType extends IlpDestinationInfo["type"]> =
  IlpDestinationInfo & { type: TType } & PreparedIlpPacketEvent

export type ResolvedPacketParameters<TType extends IlpDestinationInfo["type"]> =
  IlpDestinationInfo & { type: TType } & ResolvedIlpPacketEvent

export type PacketSender<TType extends IlpDestinationInfo["type"]> = Actor<{
  sendPrepare: (parameters: PreparedPacketParameters<TType>) => Promisable<void>
  sendResult: (parameters: ResolvedPacketParameters<TType>) => Promisable<void>
}>

type AllPacketSenders = {
  [K in IlpDestinationInfo["type"]]: PacketSender<K>
}

export const createPacketSender = (sig: ActorContext) => {
  const senders: AllPacketSenders = {
    peer: sig.use(sendPeerPackets),
    ildcp: sig.use(sendIldcpPackets),
    btp: sig.use(sendBtpPackets),
    plugin: sig.use(sendPluginPackets),
  }

  for (const sender of Object.values(senders)) {
    sender.run(sig)
  }

  return <TType extends IlpDestinationInfo["type"]>(
    client: IlpDestinationInfo & { type: TType },
    event: PreparedIlpPacketEvent | ResolvedIlpPacketEvent
  ) => {
    const sender = senders[client.type] as PacketSender<TType>

    if ("prepare" in event) {
      sender.tell("sendResult", {
        ...client,
        ...event,
      })
    } else {
      sender.tell("sendPrepare", {
        ...client,
        ...event,
      })
    }
  }
}
