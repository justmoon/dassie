import { isObject } from "@dassie/lib-type-utils"

import type { ActorContext } from "./context"
import type { Factory } from "./reactor"
import { Signal, createSignal } from "./signal"

export type Behavior<TReturn = unknown, TProperties = unknown> = (
  sig: ActorContext,
  properties: TProperties
) => TReturn

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

  behavior: Behavior<TInstance, TProperties>
}

export const createActor = <TInstance, TProperties = undefined>(
  behavior: Behavior<TInstance, TProperties>
): Actor<TInstance, TProperties> => {
  const signal = createSignal<Awaited<TInstance>>()
  const actor: Actor<TInstance, TProperties> = {
    ...signal,
    [ActorSymbol]: true,
    behavior,
  }

  return actor
}

export const isActor = (object: unknown): object is Actor<unknown, unknown> =>
  isObject(object) && object[ActorSymbol] === true
