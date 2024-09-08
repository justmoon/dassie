import type { Logger } from "@dassie/lib-logger"
import type { IlpEndpoint } from "@dassie/lib-protocol-ilp"
import type { Clock, DisposableScope } from "@dassie/lib-reactive"

import type { CryptoContext } from "../crypto/context"

export interface StreamProtocolContext {
  readonly crypto: CryptoContext
  readonly logger: Logger
  readonly endpoint: IlpEndpoint
  readonly scope: DisposableScope
  readonly clock: Clock
}
