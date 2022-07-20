import { isObject } from "@xen-ilp/lib-type-utils"

import { Topic, createTopic } from "./topic"

export type Reducer<TState> = (previousState: TState) => TState

export const StoreSymbol = Symbol("xen:reactive:topic")

export type StoreFactory<TState, TReducer = Reducer<TState>> = () => Store<
  TState,
  TReducer
>

export type Store<TState, TReducer = Reducer<TState>> = Topic<
  TState,
  TReducer
> & {
  /**
   * Marks this object as a store.
   */
  [StoreSymbol]: true

  /**
   * The current state of the store.
   */
  state: TState
}

export const createStore = <TState>(initialState: TState): Store<TState> => {
  const topic = createTopic<TState>()
  let state = initialState

  const emit = (reducer: Reducer<TState>) => {
    state = reducer(state)
    topic.emit(state)
  }

  const emitAndWait = async (reducer: Reducer<TState>) => {
    state = reducer(state)
    await topic.emitAndWait(state)
  }

  return {
    ...topic,
    emit,
    emitAndWait,
    [StoreSymbol]: true,
    get state() {
      return state
    },
  }
}

export const isStore = (object: unknown): object is Store<unknown> =>
  isObject(object) && object[StoreSymbol] === true
