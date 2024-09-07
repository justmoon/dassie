import type { ActorContext, Reactor } from "@dassie/lib-reactive"
import type { Clock, Random } from "@dassie/lib-reactive"

export interface DassieBase {
  random: Random
  clock: Clock
}

export type DassieReactor = Reactor<DassieBase>
export type DassieActorContext = ActorContext<DassieBase>
