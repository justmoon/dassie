import { isObject } from "@dassie/lib-type-utils"

import {
  ComputationContext,
  ComputationContextImplementation,
} from "./computation-context"
import { FactoryNameSymbol } from "./internal/context-base"
import { Reactive, defaultComparator } from "./internal/reactive"
import { createReactiveTopic } from "./internal/reactive-topic"
import { LifecycleScope } from "./lifecycle"
import { ReadonlySignal, SignalSymbol } from "./signal"
import { TopicSymbol } from "./topic"
import { StatefulContext } from "./types/stateful-context"

export const ComputedSymbol = Symbol("das:reactive:computed")

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

class ComputedImplementation<
  TState,
  TBase extends object,
> extends Reactive<TState> {
  [SignalSymbol] = true as const;
  [TopicSymbol] = true as const;
  [ComputedSymbol] = true as const

  private context: ComputationContext<TBase>

  constructor(
    parentContext: LifecycleScope & StatefulContext<TBase>,
    private readonly computation: (sig: ComputationContext<TBase>) => TState,
    options: ComputedOptions<TState> = {},
  ) {
    super(options.comparator ?? defaultComparator, false)

    const context = new ComputationContextImplementation(
      this[FactoryNameSymbol],
      parentContext.reactor,
      (signal) => this.readWithTracking(signal),
    )

    context.confineTo(parentContext)
    context.onCleanup(() => {
      this.removeParentObservers()
    })

    this.context = context
  }

  protected recompute() {
    this.cache = this.computation(this.context)
  }
}

export function createComputed<TState, TBase extends object>(
  parentContext: LifecycleScope & StatefulContext<TBase>,
  computation: (sig: ComputationContext<TBase>) => TState,
  options: ComputedOptions<TState> = {},
): Computed<TState> {
  const computed = new ComputedImplementation(
    parentContext,
    computation,
    options,
  )

  return Object.assign(computed, createReactiveTopic(computed))
}

export const isComputed = (object: unknown): object is Computed<unknown> =>
  isObject(object) && object[ComputedSymbol] === true
