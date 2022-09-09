import { isObject } from "@dassie/lib-type-utils"

import type { Effect } from "./effect"
import { Store, createStore } from "./store"

export const ServiceSymbol = Symbol("das:reactive:service")

export interface ServiceFactory<TInstance>
  extends Effect<never, Service<TInstance>> {
  (): Service<TInstance>
}

export type Service<TInstance> = Store<TInstance | undefined> & {
  /**
   * Marks this object as a value.
   */
  [ServiceSymbol]: true

  effect: Effect<unknown, Service<TInstance>>
}

export const createService = <T>(effect: Effect<Service<T>, T>): Service<T> => {
  const store = createStore<T>()
  const value: Service<T> = {
    ...store,
    [ServiceSymbol]: true,
    effect: (sig) => {
      value.write(sig.run(effect, value))

      sig.onCleanup(() => value.write(undefined))

      return value
    },
  }

  return value
}

export const isService = (object: unknown): object is Service<unknown> =>
  isObject(object) && object[ServiceSymbol] === true
