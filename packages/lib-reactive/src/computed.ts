import { isObject } from "@dassie/lib-type-utils"

import { type Factory, Reactor, type UseOptions } from "./reactor"
import { type ReadonlySignal, type Signal, createSignal } from "./signal"

export const ComputedSymbol = Symbol("das:reactive:computed")

export type ComputedFactory<TState> = Factory<Computed<TState>>

export type Computed<TState> = Signal<TState> & {
  /**
   * Marks this object as a signal.
   */
  [ComputedSymbol]: true
}

export interface ComputationContext {
  reactor: Reactor

  /**
   * Read the current value from a signal and lazily recompute if the value changes.
   *
   * @remarks
   *
   * To read a value without tracking, use {@link peek}.
   *
   * @param signalFactory - Reference to the signal's factory function.
   * @param selector - Used to select only part of the value from a given signal. This can be useful to avoid re-running the actor if only an irrelevant portion of the value has changed.
   * @param comparator - By default, the reactor checks for strict equality (`===`) to determine if the value has changed. This can be overridden by passing a custom comparator function.
   * @returns The current value of the signal, narrowed by the selector.
   */
  get<TState>(signalFactory: Factory<ReadonlySignal<TState>>): TState
  get<TState, TSelection>(
    signalFactory: Factory<ReadonlySignal<TState>>,
    selector: (state: TState) => TSelection,
    comparator?: (oldValue: TSelection, newValue: TSelection) => boolean
  ): TSelection

  /**
   * Read the current value from a signal but without subscribing to changes.
   *
   * @param signalFactory - Reference to the signal's factory function.
   * @returns The current value of the signal.
   */
  peek<TState>(signalFactory: Factory<ReadonlySignal<TState>>): TState

  /**
   * Fetch a context value.
   *
   * @remarks
   *
   * If the value is not found, it will be instantiated by calling the factory function.
   *
   * @param factory - Factory function corresponding to the desired value.
   * @returns - Return value of the factory function.
   */
  use<TReturn>(
    factory: Factory<TReturn>,
    options?: UseOptions | undefined
  ): TReturn
}

export function createComputed<TState>(
  computation: (sig: ComputationContext) => TState
): Computed<TState> {
  const reactor = Reactor.current

  let sources = new Set<Factory<ReadonlySignal<unknown>>>()
  const disposers = new Map<Factory<ReadonlySignal<unknown>>, () => void>()

  if (!reactor) {
    throw new Error("Computed signals must be created by a reactor.")
  }

  const context = {
    reactor,

    get<TState>(signalFactory: Factory<ReadonlySignal<TState>>) {
      sources.add(signalFactory)
      return reactor.use(signalFactory).read()
    },

    peek<TState>(signalFactory: Factory<ReadonlySignal<TState>>) {
      return reactor.use(signalFactory).read()
    },

    use<TReturn>(factory: Factory<TReturn>, options?: UseOptions | undefined) {
      return reactor.use(factory, options)
    },
  }

  let dirty = false
  const notify = () => {
    if (!dirty) {
      dirty = true
      queueMicrotask(() => {
        dirty = false
        signal.write(run())
      })
    }
  }

  const run = () => {
    const previousSources = sources
    sources = new Set()

    const result = computation(context)

    // Unsubscribe from any sources that are no longer used.
    for (const source of previousSources) {
      if (!sources.has(source)) {
        disposers.get(source)?.()
      }
    }

    // Subscribe to any new sources.
    for (const source of sources) {
      if (!previousSources.has(source)) {
        disposers.set(source, reactor.use(source).on(notify))
      }
    }

    return result
  }

  const signal = createSignal<TState>(run())

  return {
    ...signal,
    [ComputedSymbol]: true,
  }
}

export const isComputed = (object: unknown): object is Computed<unknown> =>
  isObject(object) && object[ComputedSymbol] === true
