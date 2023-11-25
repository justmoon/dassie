import { isObject } from "@dassie/lib-type-utils"

import { FactoryNameSymbol } from "./internal/context-base"
import {
  CacheStatus,
  Reactive,
  ReactiveSource,
  defaultComparator,
} from "./internal/reactive"
import { createReactiveTopic } from "./internal/reactive-topic"
import { ReadonlyTopic } from "./topic"

export type Reducer<TState> = (previousState: TState) => TState

export const SignalSymbol = Symbol("das:reactive:signal")

export type ReadonlySignal<TState> = ReadonlyTopic<TState> &
  ReactiveSource<TState> & {
    /**
     * Marks this object as a signal.
     */
    [SignalSymbol]: true

    /**
     * Name of the factory function that created this signal.
     *
     * @see {@link FactoryNameSymbol}
     */
    [FactoryNameSymbol]: string

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

export interface SignalOptions<TState> {
  comparator?: ((oldValue: TState, newValue: TState) => boolean) | undefined
}

export class SignalImplementation<TState> extends Reactive<TState> {
  [SignalSymbol] = true as const

  protected override cache: TState

  constructor(initialState: TState, options: SignalOptions<TState> = {}) {
    super(options.comparator ?? defaultComparator, false)

    this.cache = initialState
    this.cacheStatus = CacheStatus.Clean
  }

  recompute() {}

  /**
   * Apply a reducer which will accept the current state and return a new state.
   *
   * @param reducer - The reducer to apply to the state.
   * @returns The new state of the signal.
   */
  update(reducer: Reducer<TState>) {
    this.write(reducer(this.cache))
    return this.cache
  }
}

export function createSignal<TState>(): Signal<TState | undefined>
export function createSignal<TState>(
  initialState: TState,
  options?: SignalOptions<TState>,
): Signal<TState>
export function createSignal<TState>(
  initialState?: TState,
  options?: SignalOptions<TState>,
): Signal<TState> {
  const signal = new SignalImplementation(initialState as TState, options)

  return Object.assign(signal, createReactiveTopic(signal))
}

export const isSignal = (
  object: unknown,
): object is SignalImplementation<unknown> =>
  isObject(object) && object[SignalSymbol] === true
