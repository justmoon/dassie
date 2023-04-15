import { isObject } from "@dassie/lib-type-utils"

import { type ReadonlyTopic, createTopic } from "./topic"

export type Reducer<TState> = (previousState: TState) => TState

export const SignalSymbol = Symbol("das:reactive:signal")

export type ReadonlySignal<TState> = ReadonlyTopic<TState> & {
  /**
   * Marks this object as a signal.
   */
  [SignalSymbol]: true

  /**
   * Get the current state of the signal.
   */
  read(): TState
}

export type Signal<TState> = ReadonlySignal<TState> & {
  /**
   * Overwrite the current state of the signal with a new value.
   *
   * @param newState - The new state of the signal.
   */
  write(this: void, newState: TState): void

  /**
   * Apply a reducer which will accept the current state and return a new state.
   *
   * @param reducer - The reducer to apply to the state.
   * @returns The new state of the signal.
   */
  update(this: void, reducer: Reducer<TState>): TState
}

export function createSignal<TState>(): Signal<TState | undefined>
export function createSignal<TState>(initialState: TState): Signal<TState>
export function createSignal<TState>(initialState?: TState): Signal<TState> {
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
    [SignalSymbol]: true,
    read,
    write,
    update,
  }
}

export const isSignal = (object: unknown): object is Signal<unknown> =>
  isObject(object) && object[SignalSymbol] === true
