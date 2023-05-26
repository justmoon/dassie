import {
  type Actor,
  type Factory,
  createActor,
  createTopic,
} from "@dassie/lib-reactive"

import { handleInterledgerPacket } from "../handlers/interledger-packet"
import { handleLinkStateRequest } from "../handlers/link-state-request"
import { handleLinkStateUpdate } from "../handlers/link-state-update"
import { handlePeeringRequest } from "../handlers/peering-request"
import { handleSubnetModuleMessage } from "../handlers/subnet-module-message"
import type { PeerMessage } from "../peer-schema"
import { PeerState } from "../stores/node-table"

export interface IncomingPeerMessageEvent<
  TType extends PeerMessageType = PeerMessageType
> {
  message: PeerMessage & {
    content: { value: { type: TType; value: PeerMessageContent<TType> } }
  }
  authenticated: boolean
  peerState: PeerState | undefined
  asUint8Array: Uint8Array
}

export type PeerMessageType = PeerMessage["content"]["value"]["type"]

export type PeerMessageContent<T extends PeerMessageType> = Extract<
  PeerMessage["content"]["value"],
  { type: T }
>["value"]

export const incomingPeerMessageTopic = () =>
  createTopic<IncomingPeerMessageEvent>()

type AllPeerMessageHandlers = {
  [K in PeerMessageType]: Actor<{
    handle: (parameters: IncomingPeerMessageEvent<K>) => Uint8Array
  }>
}

type AllPeerMessageHandlerFactories = {
  [K in keyof AllPeerMessageHandlers]: Factory<AllPeerMessageHandlers[K]>
}

const HANDLERS: AllPeerMessageHandlerFactories = {
  peeringRequest: handlePeeringRequest,
  linkStateUpdate: handleLinkStateUpdate,
  interledgerPacket: handleInterledgerPacket,
  linkStateRequest: handleLinkStateRequest,
  subnetModuleMessage: handleSubnetModuleMessage,
}

export const handlePeerMessage = () =>
  createActor((sig) => {
    for (const handler of Object.values(HANDLERS)) {
      sig.run(handler as Factory<Actor<unknown>>, undefined, { register: true })
    }

    // eslint-disable-next-line @typescript-eslint/no-unsafe-assignment
    const handlers: AllPeerMessageHandlers = Object.fromEntries(
      Object.entries(HANDLERS).map(([key, value]) => [
        key,
        sig.use(value as Factory<unknown>),
      ])
      // eslint-disable-next-line @typescript-eslint/no-explicit-any
    ) as any

    return {
      handle: async <TType extends PeerMessageType>(
        parameters: IncomingPeerMessageEvent<TType>
      ) => {
        const type: TType = parameters.message.content.value.type

        return await handlers[type].ask("handle", parameters)
      },
    }
  })
