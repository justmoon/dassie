import { ActorContext, Reactor } from "@dassie/lib-reactive"
import type { Time } from "@dassie/lib-reactive"

export interface DassieBase {
  time: Time
}

export type DassieReactor = Reactor<DassieBase>
export type DassieActorContext = ActorContext<DassieBase>
