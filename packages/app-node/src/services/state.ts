import type { AsyncOrSync } from "ts-essentials"

export type GetState<T> = () => T
export type SetState<T, U = T> = (
  updater: (value: T) => U,
  sideEffects?: StateListener<T> | StateListener<T>[]
) => T
export type SubscribeState<T> = (listener: StateListener<T>) => () => void
export type DestroyState = () => void

export interface Store<T> {
  get: GetState<T>
  set: SetState<T>
  subscribe: SubscribeState<T>
  destroy: DestroyState
}

export type StateListener<T> = (state: T, previousState: T) => AsyncOrSync<void>

export default class State {
  createStore<T>(value: T): Store<T> {
    const listeners = new Set<StateListener<T>>()

    const get = () => value
    const set: SetState<T> = (update, sideEffects) => {
      const previousValue = value
      value = update(value)
      for (const sideEffect of typeof sideEffects === "function"
        ? [sideEffects]
        : sideEffects ?? [])
        sideEffect(value, previousValue)
      for (const listener of listeners) listener(value, previousValue)
      return value
    }
    const subscribe: SubscribeState<T> = (listener) => {
      listeners.add(listener)

      return () => listeners.delete(listener)
    }
    const destroy = () => listeners.clear()

    return { get, set, subscribe, destroy }
  }
}
