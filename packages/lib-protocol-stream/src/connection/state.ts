import type { PskEnvironment } from "../crypto/functions"
import type { StreamState } from "../stream/state"
import type { Ratio } from "../types/ratio"
import type { ConnectionContext } from "./context"

export interface ConnectionState {
  readonly context: ConnectionContext
  readonly pskEnvironment: PskEnvironment
  remoteAddress: string | undefined
  exchangeRate: Ratio | undefined
  nextSequence: number
  nextStreamId: number
  maximumPacketAmount: bigint
  readonly streams: Map<number, StreamState>
}
