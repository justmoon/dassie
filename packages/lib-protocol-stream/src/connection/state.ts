import type { IldcpResponse } from "@dassie/lib-protocol-ildcp"
import type { Topic } from "@dassie/lib-reactive"

import type { StreamProtocolContext } from "../context/context"
import type { PskEnvironment } from "../crypto/functions"
import type { StreamState } from "../stream/state"
import type { Stream } from "../stream/stream"

export interface ConnectionState {
  readonly context: StreamProtocolContext
  configuration: IldcpResponse
  readonly pskEnvironment: PskEnvironment
  remoteAddress: string | undefined
  nextSequence: number
  nextStreamId: number
  maximumPacketAmount: bigint
  readonly streams: Map<number, StreamState>
  readonly topics: {
    readonly stream: Topic<Stream>
  }
}
