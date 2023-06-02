import { Promisable } from "type-fest"

import { Actor, Factory, createActor } from "@dassie/lib-reactive"

import { BtpDestinationInfo, sendBtpPackets } from "./senders/send-btp-packets"
import {
  IldcpDestinationInfo,
  sendIldcpPackets,
} from "./senders/send-ildcp-packets"
import {
  PeerDestinationInfo,
  sendPeerPackets,
} from "./senders/send-peer-packets"
import {
  PluginDestinationInfo,
  sendPluginPackets,
} from "./senders/send-plugin-packets"
import { routingTableSignal } from "./signals/routing-table"
import {
  PreparedIlpPacketEvent,
  preparedIlpPacketTopic,
} from "./topics/prepared-ilp-packet"
import {
  ResolvedIlpPacketEvent,
  resolvedIlpPacketTopic,
} from "./topics/resolved-ilp-packet"

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

type AllPacketSenderFactories = {
  [K in keyof AllPacketSenders]: Factory<AllPacketSenders[K]>
}

const SENDERS: AllPacketSenderFactories = {
  peer: sendPeerPackets,
  ildcp: sendIldcpPackets,
  btp: sendBtpPackets,
  plugin: sendPluginPackets,
}

export const sendOutgoingPackets = () =>
  createActor((sig) => {
    const globalIlpClientMap = sig.use(routingTableSignal)

    // eslint-disable-next-line @typescript-eslint/no-unsafe-assignment
    const senders: AllPacketSenders = Object.fromEntries(
      Object.entries(SENDERS).map(([key, value]) => [
        key,
        sig.use(value as Factory<unknown>),
      ])
      // eslint-disable-next-line @typescript-eslint/no-explicit-any
    ) as any

    for (const sender of Object.values(senders)) {
      sender.run(sig)
    }

    const getDestinationInfo = (
      destination: string
    ): IlpDestinationInfo | undefined => {
      return globalIlpClientMap.read().lookup(destination)
    }

    const callSendPrepare = <TType extends IlpDestinationInfo["type"]>(
      client: IlpDestinationInfo & { type: TType },
      event: PreparedIlpPacketEvent
    ) => {
      const sender = senders[client.type] as PacketSender<TType>

      sender.tell("sendPrepare", {
        ...client,
        ...event,
      })
    }

    const callSendResult = <TType extends IlpDestinationInfo["type"]>(
      client: IlpDestinationInfo & { type: TType },
      event: ResolvedIlpPacketEvent
    ) => {
      const sender = senders[client.type] as PacketSender<TType>

      sender.tell("sendResult", {
        ...client,
        ...event,
      })
    }

    sig.on(preparedIlpPacketTopic, (event) => {
      const client = getDestinationInfo(event.parsedPacket.destination)

      if (!client) {
        throw new Error(
          `No routing table entry found for ${event.parsedPacket.destination}`
        )
      }

      callSendPrepare(client, event)
    })

    sig.on(resolvedIlpPacketTopic, (event) => {
      const client = getDestinationInfo(event.prepare.sourceIlpAddress)

      if (!client) {
        throw new Error(
          `No routing table entry found for ${event.prepare.sourceIlpAddress}`
        )
      }

      callSendResult(client, event)
    })
  })
