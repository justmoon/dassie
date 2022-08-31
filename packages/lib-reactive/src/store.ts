import { isObject } from "@dassie/lib-type-utils"

import type { Factory } from "./reactor"
import { Topic, createTopic } from "./topic"

export type Reducer<TState> = (previousState: TState) => TState

export const StoreSymbol = Symbol("das:reactive:store")

export interface StoreFactory<TState, TReducer = Reducer<TState>>
  extends Factory<Store<TState, TReducer>> {
  (): Store<TState, TReducer>
}

export type Store<TState, TReducer = Reducer<TState>> = Topic<
  TState,
  Readonly<TState>
> & {
  /**
   * Marks this object as a store.
   */
  [StoreSymbol]: true

  /**
   * Get the current state of the store.
   */
  read(): TState

  /**
   * Overwrite the current state of the store with a new value.
   *
   * @param newState - The new state of the store.
   */
  write(newState: TState): void

  /**
   * Apply a reducer which will accept the current state and return a new state.
   *
   * @param reducer - The reducer to apply to the state.
   * @returns The new state of the store.
   */
  update(reducer: TReducer): TState
}

export function createStore<TState>(): Store<TState | undefined>
export function createStore<TState>(initialState: TState): Store<TState>
export function createStore<TState>(initialState?: TState): Store<TState> {
  const topic = createTopic<TState>()
  let currentState = initialState as TState

  const read = () => currentState

  const write = (newState: TState) => {
    currentState = newState
    topic.emit(newState)
  }

  const update = (reducer: Reducer<TState>) => {
    currentState = reducer(currentState)
    topic.emit(currentState)
    return currentState
  }

  return {
    ...topic,
    [StoreSymbol]: true,
    read,
    write,
    update,
  }
}

export const isStore = (object: unknown): object is Store<unknown> =>
  isObject(object) && object[StoreSymbol] === true
