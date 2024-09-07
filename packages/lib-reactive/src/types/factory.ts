import type { Reactor } from "../reactor"

export type Factory<TInstance, TBase extends object = object> = (
  reactor: Reactor<TBase>,
) => TInstance
