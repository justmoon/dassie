import { isObject } from "@dassie/lib-type-utils"

import { defaultSelector } from "./actor-context"
import {
  Reactive,
  ReactiveSource,
  defaultComparator,
} from "./internal/reactive"
import { createReactiveTopic } from "./internal/reactive-topic"
import { LifecycleScope } from "./lifecycle"
import { ReadonlySignal, SignalSymbol } from "./signal"
import { TopicSymbol } from "./topic"

export const ComputedSymbol = Symbol("das:reactive:computed")

export interface ComputationContext {
  /**
   * Read the current value from a signal and lazily recompute if the value changes.
   *
   * @param signal - Reference to the signal.
   * @param selector - Used to select only part of the value from a given signal. This can be useful to avoid re-running the actor if only an irrelevant portion of the value has changed.
   * @param comparator - By default, the reactor checks for strict equality (`===`) to determine if the value has changed. This can be overridden by passing a custom comparator function.
   * @returns The current value of the signal, narrowed by the selector.
   */
  get<TState>(signal: ReactiveSource<TState>): TState
  get<TState, TSelection>(
    signal: ReactiveSource<TState>,
    selector: (state: TState) => TSelection,
    comparator?: (oldValue: TSelection, newValue: TSelection) => boolean,
  ): TSelection

  /**
   * Convenience method for extracting specific keys from a signal.
   *
   * @remarks
   *
   * This method works like {@link get} but will automatically create the correct selector and comparator for the given keys. The value will be recomputed if any of the values for any of the keys change by strict equality.
   *
   * @param signal - Reference to the signal that should be queried.
   * @param keys - Tuple of keys that should be extracted from the signal.
   * @returns A filtered version of the signal state containing only the requested keys.
   */
  getKeys<TState, TKeys extends keyof TState>(
    signal: ReactiveSource<TState>,
    keys: readonly TKeys[],
  ): Pick<TState, TKeys>

  /**
   * The lifetime of the computation.
   */
  lifecycle: LifecycleScope
}

export interface Computed<TState> extends ReadonlySignal<TState> {
  /**
   * Marks this object as a computed value.
   */
  [ComputedSymbol]: true

  /**
   * Asynchronously update the value of the computation.
   */
  write(value: TState): void
}

export interface ComputedOptions<TState> {
  comparator?: ((oldValue: TState, newValue: TState) => boolean) | undefined
}

class ComputedImplementation<TState> extends Reactive<TState> {
  [SignalSymbol] = true as const;
  [TopicSymbol] = true as const;
  [ComputedSymbol] = true as const

  onCleanup: LifecycleScope["onCleanup"]

  constructor(
    readonly lifecycle: LifecycleScope,
    private readonly computation: (sig: ComputationContext) => TState,
    options: ComputedOptions<TState> = {},
  ) {
    super(options.comparator ?? defaultComparator, false)

    lifecycle.onCleanup(() => {
      this.removeParentObservers()
    })
    this.onCleanup = lifecycle.onCleanup
  }

  protected recompute() {
    const context: ComputationContext = {
      get: <T>(
        signal: ReactiveSource<T>,
        selector = defaultSelector,
        comparator = defaultComparator,
      ) => {
        return selector === defaultSelector && comparator === defaultComparator
          ? this.readWithTracking(signal)
          : this.readWithTracking(signal, selector, comparator)
      },
      getKeys: <TState, TKeys extends keyof TState>(
        signal: ReactiveSource<TState>,
        keys: readonly TKeys[],
      ) => {
        return context.get(
          signal,
          (state) => {
            const result = {} as Pick<TState, TKeys>
            for (const key of keys) {
              result[key] = state[key]
            }
            return result
          },
          (a, b) => {
            for (const key of keys) {
              if (a[key] !== b[key]) {
                return false
              }
            }
            return true
          },
        )
      },
      lifecycle: this.lifecycle,
    }
    this.cache = this.computation(context)
  }
}

export function createComputed<TState>(
  lifecycle: LifecycleScope,
  computation: (sig: ComputationContext) => TState,
  options: ComputedOptions<TState> = {},
): Computed<TState> {
  const computed = new ComputedImplementation(lifecycle, computation, options)

  return Object.assign(computed, createReactiveTopic(computed))
}

export const isComputed = (object: unknown): object is Computed<unknown> =>
  isObject(object) && object[ComputedSymbol] === true
