import type { ActorContext, Reactor } from "@dassie/lib-reactive"
import type { Clock, Crypto } from "@dassie/lib-reactive"

export interface DassieBase {
  clock: Clock
  crypto: Crypto
}

export type DassieReactor = Reactor<DassieBase>
export type DassieActorContext = ActorContext<DassieBase>
