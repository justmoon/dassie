import { isObject } from "@dassie/lib-type-utils"

import { ActorFactory, createActor } from "./actor"
import type { Effect } from "./effect"
import { Factory, FactoryNameSymbol } from "./reactor"
import { Signal, createSignal } from "./signal"

export const ServiceSymbol = Symbol("das:reactive:service")

export type ServiceFactory<TInstance, TProperties = undefined> = Factory<
  Service<TInstance, TProperties>
>

export type Service<TInstance, TProperties = undefined> = Signal<
  TInstance | undefined
> & {
  /**
   * Marks this object as a value.
   */
  [ServiceSymbol]: true

  effect: ActorFactory<Service<TInstance, TProperties>, TProperties>
}

export const createService = <TInstance, TProperties = undefined>(
  effect: Effect<TProperties, TInstance | undefined>
): Service<TInstance, TProperties> => {
  const signal = createSignal<TInstance>()
  const subActor = () => createActor(effect)

  const service: Service<TInstance, TProperties> = {
    ...signal,
    [ServiceSymbol]: true,
    effect: () =>
      createActor((sig, properties) => {
        sig.run(subActor, properties, {
          additionalDebugData: {
            service: service[FactoryNameSymbol],
          },
          onResult: (result) => {
            service.write(result)
          },
        })

        sig.onCleanup(() => service.write(undefined))

        return service
      }),
  }

  return service
}

export const isService = (
  object: unknown
): object is Service<unknown, unknown> =>
  isObject(object) && object[ServiceSymbol] === true
