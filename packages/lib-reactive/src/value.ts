import { isObject } from "@dassie/lib-type-utils"

import { Effect, runEffect } from "./effect"
import { FactoryNameSymbol, InitSymbol, Reactor } from "./reactor"
import { Store, createStore } from "./store"

export const ValueSymbol = Symbol("das:reactive:value")

export interface ValueFactory<TInstance>
  extends Effect<never, Value<TInstance>> {
  (): Value<TInstance>
}

export type Value<TInstance> = Store<TInstance> & {
  /**
   * Marks this object as a value.
   */
  [ValueSymbol]: true

  [InitSymbol]: (reactor: Reactor) => void
}

export const createValue = <T>(effect: Effect<undefined, T>): Value<T> => {
  // It's ok to temporarily put undefined in the store since we will immediately replace it in the initialization function
  const store = createStore<T>(undefined as unknown as T)
  const value: Value<T> = {
    ...store,
    [ValueSymbol]: true,
    [InitSymbol]: (reactor: Reactor) => {
      runEffect(reactor, effect, undefined, reactor, (newValue) => {
        if (store.read() !== newValue) {
          store.emit(() => newValue)
        }
      }).catch((error: unknown) => {
        console.error("error while running effect to calculate value", {
          value: value[FactoryNameSymbol],
          error,
        })
      })
    },
  }

  return value
}

export const isValue = (object: unknown): object is Value<unknown> =>
  isObject(object) && object[ValueSymbol] === true
