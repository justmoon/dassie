import { isObject } from "@dassie/lib-type-utils"

import { FactoryNameSymbol } from "./internal/context-base"
import {
  CacheStatus,
  Reactive,
  type ReactiveSource,
  defaultComparator,
} from "./internal/reactive"
import { createReactiveTopic } from "./internal/reactive-topic"
import type { ReadonlyTopic } from "./topic"
import type { Factory } from "./types/factory"

export type Reducer<TState> = (previousState: TState) => TState

export const SignalSymbol = Symbol("das:reactive:signal")

export type InferSignalType<
  TTopic extends ReadonlySignal<unknown> | Factory<ReadonlySignal<unknown>>,
> =
  TTopic extends ReadonlySignal<infer TMessage> ? TMessage
  : TTopic extends Factory<ReadonlySignal<infer TMessage>> ? TMessage
  : never

export interface ReadonlySignal<TState> extends ReactiveSource<TState> {
  /**
   * Marks this object as a signal.
   */
  [SignalSymbol]: true

  /**
   * Name of the factory function that created this signal.
   *
   * @see {@link FactoryNameSymbol}
   */
  [FactoryNameSymbol]: string | undefined

  /**
   * Get the current state of the signal.
   */
  read(): TState

  /**
   * A topic that will emit each new value of the signal.
   */
  values: ReadonlyTopic<TState>
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

  values: ReadonlyTopic<TState>

  constructor(initialState: TState, options: SignalOptions<TState> = {}) {
    super(options.comparator ?? defaultComparator, false)

    this.cache = initialState
    this.cacheStatus = CacheStatus.Clean

    this.values = createReactiveTopic(this)
  }

  recompute() {
    // no-op
  }

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
  return new SignalImplementation(initialState as TState, options)
}

export const isSignal = (object: unknown): object is Signal<unknown> =>
  isObject(object) && object[SignalSymbol] === true
