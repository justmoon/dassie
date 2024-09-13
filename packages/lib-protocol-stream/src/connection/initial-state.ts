import { UINT64_MAX } from "@dassie/lib-oer"
import type { IldcpResponse } from "@dassie/lib-protocol-ildcp"
import { createTopic } from "@dassie/lib-reactive"

import type { StreamProtocolContext } from "../context/context"
import { getPskEnvironment } from "../crypto/functions"
import type { ConnectionState } from "./state"

export const DEFAULT_MAXIMUM_PACKET_AMOUNT = UINT64_MAX

export const INITIAL_CONCURRENCY = 1

export interface ConnectionStateOptions {
  context: StreamProtocolContext
  configuration: IldcpResponse
  ourAddress: string
  remoteAddress?: string | undefined
  secret: Uint8Array
  side: "server" | "client"
}

export const createInitialConnectionState = ({
  context,
  configuration,
  ourAddress,
  remoteAddress,
  secret,
  side,
}: ConnectionStateOptions): ConnectionState => {
  return {
    context,
    side,
    configuration,
    ourAddress,
    remoteAddress,
    pskEnvironment: getPskEnvironment(context.crypto, secret),
    nextSequence: 0,
    // Stream IDs are odd for clients and even for servers
    nextStreamId: side === "client" ? 1 : 2,
    maxStreamId: side === "client" ? 2 : 1,
    remoteMaxStreamId: undefined,
    remoteAssetDetails: undefined,
    streams: new Map(),
    maxPacketAmount: DEFAULT_MAXIMUM_PACKET_AMOUNT,
    topics: {
      stream: createTopic(),
    },
    isSending: false,
    concurrency: INITIAL_CONCURRENCY,
    sendLoopWaker: undefined,
    exchangeRate: undefined,
    remoteKnowsAddress: side === "server",
  }
}
