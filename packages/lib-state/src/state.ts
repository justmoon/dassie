import type { AsyncOrSync } from "ts-essentials"

export type GetState<T> = () => T
export type SetState<T, U = T> = (
  updater: (value: T) => U,
  sideEffects?: StateListener<T> | StateListener<T>[]
) => T
export type SubscribeState<T> = (listener: StateListener<T>) => () => void
export type DestroyState = () => void

export interface Store<T, U = T> {
  get: GetState<T>
  set: SetState<T, U>
  subscribe: SubscribeState<T>
  destroy: DestroyState
}

export type StateListener<T> = (state: T, previousState: T) => AsyncOrSync<void>

export interface StateOptions {
  errorHandler?: (error: unknown) => void
}

export default class State {
  readonly stores = new Map<string, WeakRef<Store<unknown, never>>>()

  constructor(readonly options: StateOptions = {}) {}

  createStore<T>(id: string, value: T): Store<T> {
    const listeners = new Set<StateListener<T>>()

    const get = () => value
    const set: SetState<T> = (update, sideEffects) => {
      const previousValue = value
      value = update(value)
      for (const sideEffect of typeof sideEffects === "function"
        ? [sideEffects]
        : sideEffects ?? []) {
        Promise.resolve(sideEffect(value, previousValue)).catch(
          this.options.errorHandler ?? console.error
        )
      }
      for (const listener of listeners) {
        Promise.resolve(listener(value, previousValue)).catch(
          this.options.errorHandler ?? console.error
        )
      }
      return value
    }
    const subscribe: SubscribeState<T> = (listener) => {
      listeners.add(listener)

      return () => listeners.delete(listener)
    }
    const destroy = () => listeners.clear()

    const store: Store<T> = { get, set, subscribe, destroy }

    this.stores.set(id, new WeakRef(store))

    return store
  }
}
