import { isObject } from "@dassie/lib-type-utils"

import {
  ComputationContext,
  ComputationContextImplementation,
} from "./computation-context"
import { FactoryNameSymbol } from "./internal/context-base"
import { Reactive, defaultComparator } from "./internal/reactive"
import { createReactiveTopic } from "./internal/reactive-topic"
import { createLifecycleScope } from "./lifecycle"
import { ReadonlySignal, SignalSymbol } from "./signal"
import { type ReadonlyTopic, TopicSymbol } from "./topic"
import { LifecycleContext } from "./types/lifecycle-context"
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

  values: ReadonlyTopic<TState>

  constructor(
    parentContext: LifecycleContext & StatefulContext<TBase>,
    private readonly computation: (sig: ComputationContext<TBase>) => TState,
    options: ComputedOptions<TState> = {},
  ) {
    super(options.comparator ?? defaultComparator, false)

    const lifecycle = createLifecycleScope(this[FactoryNameSymbol])

    lifecycle.confineTo(parentContext.lifecycle)
    lifecycle.onCleanup(() => {
      this.removeParentObservers()
    })

    const context = new ComputationContextImplementation(
      lifecycle,
      parentContext.reactor,
      (signal) => this.readWithTracking(signal),
    )

    this.context = context
    this.values = createReactiveTopic(this)
  }

  protected recompute() {
    this.cache = this.computation(this.context)
  }
}

export function createComputed<TState, TBase extends object>(
  parentContext: LifecycleContext & StatefulContext<TBase>,
  computation: (sig: ComputationContext<TBase>) => TState,
  options: ComputedOptions<TState> = {},
): Computed<TState> {
  return new ComputedImplementation(parentContext, computation, options)
}

export const isComputed = (object: unknown): object is Computed<unknown> =>
  isObject(object) && object[ComputedSymbol] === true
