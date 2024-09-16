import type { IldcpResponse } from "@dassie/lib-protocol-ildcp"
import type { Deferred, DisposableScope } from "@dassie/lib-reactive"

import type { StreamProtocolContext } from "../context/context"
import type { PskEnvironment } from "../crypto/functions"
import type { Ratio } from "../math/ratio"
import type { StreamState } from "../stream/state"
import type { Stream } from "../stream/stream"
import type { InferTopics } from "../types/infer-topics"

export type ConnectionEvents = {
  stream: Stream
  closed: void
}

export interface ConnectionState {
  readonly context: StreamProtocolContext
  readonly scope: DisposableScope
  side: "server" | "client"
  configuration: IldcpResponse
  readonly pskEnvironment: PskEnvironment
  ourAddress: string
  remoteAddress: string | undefined
  nextSequence: number
  nextStreamId: number
  maxPacketAmount: bigint
  maxStreamId: number
  remoteMaxStreamId: number | undefined
  remoteAssetDetails:
    | Pick<IldcpResponse, "assetCode" | "assetScale">
    | undefined
  readonly streams: Map<number, StreamState>
  readonly closedStreams: Map<number, StreamState>
  readonly topics: InferTopics<ConnectionEvents>
  sendLoopPromise: Promise<void> | undefined

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

  /**
   * Whether the other side knows our ILP address.
   */
  remoteKnowsAddress: boolean

  /**
   * Whether we consider the connection to be closed.
   */
  isClosed: boolean
}
