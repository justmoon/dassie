import type { Clock, Random } from "@dassie/lib-reactive"

export interface Runtime {
  readonly random: Random
  readonly clock: Clock
}
