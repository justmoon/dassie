import { isObject } from "@dassie/lib-type-utils"

import { Effect, runEffect } from "./effect"
import { FactoryNameSymbol } from "./reactor"
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
      runEffect(sig.reactor, effect, service, sig.lifecycle, (_result) =>
        service.write(_result)
      ).catch((error: unknown) => {
        console.error("error in service effect", {
          effect: effect.name,
          service: service[FactoryNameSymbol],
          error,
        })
      })

      sig.onCleanup(() => service.write(undefined))

      return service
    },
  }

  return service
}

export const isService = (object: unknown): object is Service<unknown> =>
  isObject(object) && object[ServiceSymbol] === true
