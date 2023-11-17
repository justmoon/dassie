import { Reactor } from "../reactor"

export type Factory<TInstance> = (reactor: Reactor) => TInstance
