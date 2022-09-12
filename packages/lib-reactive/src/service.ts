import { isObject } from "@dassie/lib-type-utils"

import { Effect, runEffect } from "./effect"
import { Factory, FactoryNameSymbol } from "./reactor"
import { Signal, createSignal } from "./signal"

export const ServiceSymbol = Symbol("das:reactive:service")

export type ServiceFactory<TInstance, TProperties> = Factory<
  Service<TInstance, TProperties>
>

export type Service<TInstance, TProperties> = Signal<TInstance | undefined> & {
  /**
   * Marks this object as a value.
   */
  [ServiceSymbol]: true

  effect: Effect<TProperties, Service<TInstance, TProperties>>
}

export const createService = <TInstance, TProperties>(
  effect: Effect<TProperties, TInstance>
): Service<TInstance, TProperties> => {
  const signal = createSignal<TInstance>()
  const service: Service<TInstance, TProperties> = {
    ...signal,
    [ServiceSymbol]: true,
    effect: (sig, properties) => {
      runEffect(sig.reactor, effect, properties, sig.lifecycle, (_result) =>
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

export const isService = (
  object: unknown
): object is Service<unknown, unknown> =>
  isObject(object) && object[ServiceSymbol] === true
