import { isObject } from "@dassie/lib-type-utils"

import type { Effect } from "./effect"
import { FactoryNameSymbol } from "./reactor"
import { Signal, createSignal } from "./signal"

export const ServiceSymbol = Symbol("das:reactive:service")

export type ServiceFactory<TInstance, TProperties = undefined> = Effect<
  undefined,
  Service<TInstance, TProperties>
>

export type Service<TInstance, TProperties = undefined> = Signal<
  TInstance | undefined
> & {
  /**
   * Marks this object as a value.
   */
  [ServiceSymbol]: true

  effect: Effect<TProperties, Service<TInstance, TProperties>>
}

export const createService = <TInstance, TProperties = undefined>(
  effect: Effect<TProperties, TInstance | undefined>
): Service<TInstance, TProperties> => {
  const signal = createSignal<TInstance>()
  const service: Service<TInstance, TProperties> = {
    ...signal,
    [ServiceSymbol]: true,
    effect: (sig, properties) => {
      sig.run(effect, properties, {
        additionalDebugData: {
          service: service[FactoryNameSymbol],
        },
        onResult: (result) => {
          service.write(result)
        },
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
