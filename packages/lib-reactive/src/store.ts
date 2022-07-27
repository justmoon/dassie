import { isObject } from "@xen-ilp/lib-type-utils"

import type { Factory } from "./reactor"
import { Topic, createTopic } from "./topic"

export type Reducer<TState> = (previousState: TState) => TState

export const StoreSymbol = Symbol("xen:reactive:store")

export interface StoreFactory<TState, TReducer = Reducer<TState>>
  extends Factory<Store<TState, TReducer>> {
  (): Store<TState, TReducer>
}

export type Store<TState, TReducer = Reducer<TState>> = Topic<
  TState,
  TReducer
> & {
  /**
   * Marks this object as a store.
   */
  [StoreSymbol]: true

  /**
   * Get the current state of the store.
   */
  read(): TState
}

export const createStore = <TState>(initialState: TState): Store<TState> => {
  const topic = createTopic<TState>()
  let currentState = initialState

  const emit = (reducer: Reducer<TState>) => {
    currentState = reducer(currentState)
    topic.emit(currentState)
  }

  const read = () => currentState

  return {
    ...topic,
    emit,
    [StoreSymbol]: true,
    read,
  }
}

export const isStore = (object: unknown): object is Store<unknown> =>
  isObject(object) && object[StoreSymbol] === true
