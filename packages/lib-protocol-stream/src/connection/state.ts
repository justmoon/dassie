import type { IldcpResponse } from "@dassie/lib-protocol-ildcp"
import type { Deferred } from "@dassie/lib-reactive"

import type { StreamProtocolContext } from "../context/context"
import type { PskEnvironment } from "../crypto/functions"
import type { Ratio } from "../math/ratio"
import type { StreamState } from "../stream/state"
import type { Stream } from "../stream/stream"
import type { InferTopics } from "../types/infer-topics"

export type ConnectionEvents = {
  stream: Stream
}

export interface ConnectionState {
  readonly context: StreamProtocolContext
  side: "server" | "client"
  configuration: IldcpResponse
  readonly pskEnvironment: PskEnvironment
  remoteAddress: string | undefined
  nextSequence: number
  nextStreamId: number
  maximumPacketAmount: bigint
  maximumStreamId: number
  readonly streams: Map<number, StreamState>
  readonly topics: InferTopics<ConnectionEvents>
  isSending: boolean

  /**
   * Number of packets that may be in flight at the same time.
   */
  concurrency: number

  /**
   * A deferred promise which can be used to wake up the send loop if there is
   * new work to do.
   */
  sendLoopWaker: Deferred<void> | undefined

  exchangeRate: Ratio | undefined
}
