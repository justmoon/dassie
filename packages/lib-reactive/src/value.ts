import { isObject } from "@dassie/lib-type-utils"

import { Effect, runEffect } from "./effect"
import { LifecycleScope } from "./internal/lifecycle-scope"
import {
  AsyncDisposer,
  DisposeSymbol,
  FactoryNameSymbol,
  InitSymbol,
  Reactor,
} from "./reactor"
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
  [DisposeSymbol]: AsyncDisposer
}

/**
 * Symbol which is used as the initial value for the store. This should never be visible to the user, since the store is immediately initialized by running the effect.
 *
 * The only way to get this value from the store is to access it from within the effect that initializes it which is a circular dependency and therefore a bug.
 *
 * To catch this case, we override the store's read() method and throw an error if it is called before the store has been initialized.
 */
const UninitializedSymbol = Symbol("das:reactive:uninitialized")

export const createValue = <T>(effect: Effect<Value<T>, T>): Value<T> => {
  const store = createStore<T>(UninitializedSymbol as unknown as T)
  const lifecycle = new LifecycleScope()
  const value: Value<T> = {
    ...store,
    read: () => {
      throw new Error(
        "Value has not been initialized. This indicates that you tried to read this value during the execution of its own initializer, i.e. a circular dependency."
      )
    },
    [ValueSymbol]: true,
    [InitSymbol]: (reactor: Reactor) => {
      reactor.onCleanup(async () => {
        await lifecycle.dispose()
      })
      runEffect(reactor, effect, value, lifecycle, (newValue) => {
        // We had temporarily overridden the read method to catch circular dependencies. Restore it now.
        // eslint-disable-next-line @typescript-eslint/unbound-method
        if (value.read !== store.read) value.read = store.read

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
    [DisposeSymbol]: () => {
      return lifecycle.dispose()
    },
  }

  return value
}

export const isValue = (object: unknown): object is Value<unknown> =>
  isObject(object) && object[ValueSymbol] === true
