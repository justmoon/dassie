import { isObject } from "@dassie/lib-type-utils"

import type { Effect } from "./effect"
import { Signal, createSignal } from "./signal"

export const ServiceSymbol = Symbol("das:reactive:service")

export interface ServiceFactory<TInstance>
  extends Effect<never, Service<TInstance>> {
  (): Service<TInstance>
}

export type Service<TInstance> = Signal<TInstance | undefined> & {
  /**
   * Marks this object as a value.
   */
  [ServiceSymbol]: true

  effect: Effect<unknown, Service<TInstance>>
}

export const createService = <T>(effect: Effect<Service<T>, T>): Service<T> => {
  const signal = createSignal<T>()
  const service: Service<T> = {
    ...signal,
    [ServiceSymbol]: true,
    effect: (sig) => {
      service.write(sig.run(effect, service))

      sig.onCleanup(() => service.write(undefined))

      return service
    },
  }

  return service
}

export const isService = (object: unknown): object is Service<unknown> =>
  isObject(object) && object[ServiceSymbol] === true
