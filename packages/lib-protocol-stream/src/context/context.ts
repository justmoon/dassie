import type { Logger } from "@dassie/lib-logger"
import type { IlpEndpoint } from "@dassie/lib-protocol-ilp"
import type { Clock, Crypto, DisposableScope } from "@dassie/lib-reactive"

import type { StreamPolicy } from "./policy"

export interface StreamProtocolContext {
  readonly crypto: Crypto
  readonly logger: Logger
  readonly endpoint: IlpEndpoint
  readonly scope: DisposableScope
  readonly clock: Clock
  readonly policy: StreamPolicy
}
