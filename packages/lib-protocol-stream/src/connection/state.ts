import type { PskEnvironment } from "../crypto/functions"
import type { Ratio } from "../types/ratio"
import type { ConnectionContext } from "./context"

export interface ConnectionState {
  readonly context: ConnectionContext
  readonly pskEnvironment: PskEnvironment
  remoteAddress: string | undefined
  exchangeRate: Ratio | undefined
  nextSequence: number
}
