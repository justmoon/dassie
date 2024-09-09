import type { Clock, Crypto } from "@dassie/lib-reactive"

export interface Runtime {
  readonly clock: Clock
  readonly crypto: Crypto
}
