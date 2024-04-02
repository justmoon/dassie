import { ActorContext, Reactor } from "@dassie/lib-reactive"
import type { Random, Time } from "@dassie/lib-reactive"

export interface DassieBase {
  random: Random
  time: Time
}

export type DassieReactor = Reactor<DassieBase>
export type DassieActorContext = ActorContext<DassieBase>
