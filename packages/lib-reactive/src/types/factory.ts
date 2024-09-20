import type { Reactor } from "../reactor"

export type Factory<TInstance, TBase extends object = object> = (
  reactor: Reactor<TBase>,
) => TInstance

export type FactoryOrInstance<TInstance, TBase extends object = object> =
  | TInstance
  | Factory<TInstance, TBase>
