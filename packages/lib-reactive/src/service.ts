import { isObject } from "@dassie/lib-type-utils"

import { Effect, runEffect } from "./effect"
import { LifecycleScope } from "./internal/lifecycle-scope"
import {
  AsyncDisposer,
  DisposeSymbol,
  FactoryNameSymbol,
  InitSymbol,
  Reactor,
  UseSymbol,
} from "./reactor"
import { Store, createStore } from "./store"

export const ServiceSymbol = Symbol("das:reactive:service")

export interface ServiceFactory<TInstance>
  extends Effect<never, Service<TInstance>> {
  (): Service<TInstance>
}

export type Service<TInstance> = Store<TInstance> & {
  /**
   * Marks this object as a value.
   */
  [ServiceSymbol]: true

  [InitSymbol]: (reactor: Reactor) => void
  [UseSymbol]: () => void
  [DisposeSymbol]: AsyncDisposer
}

const readUninitialized = () => {
  throw new Error(
    "Service has not been initialized. This indicates that you tried to read this value during the execution of its own initializer or destructor, i.e. a circular dependency."
  )
}

/**
 * Symbol which is used as the initial value for the store. This should never be visible to the user, since the store is immediately initialized by running the effect.
 *
 * The only way to get this value from the store is to access it from within the effect that initializes it which is a circular dependency and therefore a bug.
 *
 * To catch this case, we override the store's read() method and throw an error if it is called before the store has been initialized.
 */
const UninitializedSymbol = Symbol("das:reactive:uninitialized")

export const createService = <T>(effect: Effect<Service<T>, T>): Service<T> => {
  const store = createStore<T>(UninitializedSymbol as unknown as T)
  let lifecycle: LifecycleScope
  let reactor!: Reactor
  let referenceCount = 0
  const value: Service<T> = {
    ...store,
    read: readUninitialized,
    [ServiceSymbol]: true,
    [InitSymbol]: (_reactor: Reactor) => {
      reactor = _reactor
    },
    [UseSymbol]: () => {
      referenceCount++

      if (referenceCount > 1) return

      lifecycle = new LifecycleScope()

      reactor.onCleanup(async () => {
        await lifecycle.dispose()
      })

      runEffect(reactor, effect, value, lifecycle, (newValue) => {
        // We had temporarily overridden the read method to catch circular dependencies. Restore it now.
        // eslint-disable-next-line @typescript-eslint/unbound-method
        if (value.read !== store.read) value.read = store.read

        if (store.read() !== newValue) {
          store.write(newValue)
        }
      }).catch((error: unknown) => {
        console.error("error while running effect to calculate value", {
          value: value[FactoryNameSymbol],
          error,
        })
      })
    },
    [DisposeSymbol]: () => {
      referenceCount--

      if (referenceCount === 0) {
        store.write(UninitializedSymbol as unknown as T)
        value.read = readUninitialized
        return lifecycle.dispose()
      }

      return
    },
  }

  return value
}

export const isService = (object: unknown): object is Service<unknown> =>
  isObject(object) && object[ServiceSymbol] === true
