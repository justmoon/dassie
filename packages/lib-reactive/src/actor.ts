import { isObject } from "@dassie/lib-type-utils"

import type { Effect } from "./effect"
import type { Factory } from "./reactor"
import { Signal, createSignal } from "./signal"

export const ActorSymbol = Symbol("das:reactive:actor")

export type ActorFactory<TInstance, TProperties = undefined> = Factory<
  Actor<TInstance, TProperties>
>

export type Actor<TInstance, TProperties = undefined> = Signal<
  Awaited<TInstance> | undefined
> & {
  /**
   * Marks this object as a value.
   */
  [ActorSymbol]: true

  effect: Effect<TProperties, TInstance | undefined>
}

export const createActor = <TInstance, TProperties = undefined>(
  effect: Effect<TProperties, TInstance | undefined>
): Actor<TInstance, TProperties> => {
  const signal = createSignal<Awaited<TInstance>>()
  const actor: Actor<TInstance, TProperties> = {
    ...signal,
    [ActorSymbol]: true,
    effect,
  }

  return actor
}

export const isActor = (object: unknown): object is Actor<unknown, unknown> =>
  isObject(object) && object[ActorSymbol] === true
