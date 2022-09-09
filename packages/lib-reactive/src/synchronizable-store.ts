import { isObject } from "@dassie/lib-type-utils"

import type { Factory, Reactor } from "./reactor"
import { Reducer, Signal, createSignal } from "./signal"
import { Topic, createTopic } from "./topic"

export const SynchronizableStoreSymbol = Symbol(
  "das:reactive:synchronizable-store"
)

export interface SynchronizableStoreFactory<
  TState = unknown,
  TActions extends Record<string, Action<TState>> = Record<
    string,
    Action<TState>
  >
> extends Factory<SynchronizableStore<TState, TActions>> {
  (reactor: Reactor): SynchronizableStore<TState, TActions>
}

export type Action<
  TState = unknown,
  TParameters extends unknown[] = never[]
> = (...parameters: TParameters) => (previousState: TState) => TState

export type Change = [string, unknown[]]

export type InferBoundAction<TAction> = TAction extends Action<infer S, infer P extends unknown[]>
  ? BoundAction<S, P>
  : never

export type InferBoundActions<TActions> = {
  [key in keyof TActions]: InferBoundAction<TActions[key]>
}

export type BoundAction<TState, TParameters extends unknown[]> = (
  ...parameters: TParameters
) => TState

const bindActions = <TState, TActions extends Record<string, Action<TState>>>(
  actions: TActions,
  store: Signal<TState>,
  changesTopic: Topic<Change, Change>
): InferBoundActions<TActions> => {
  const boundActions = {} as InferBoundActions<TActions>

  for (const key in actions) {
    if (Object.prototype.hasOwnProperty.call(actions, key)) {
      boundActions[key] = ((...parameters) => {
        const reducer = actions[key]!(...parameters)

        store.update(reducer)
        changesTopic.emit([key, parameters])

        return store.read()
      }) as typeof boundActions[typeof key]
    }
  }

  return boundActions
}

export type SynchronizableStore<
  TState,
  TActions extends Record<string, Action<TState>>
> = Signal<TState, Reducer<TState>> & {
  /**
   * Marks this object as a store.
   */
  [SynchronizableStoreSymbol]: true

  changes: Topic<Change>
} & InferBoundActions<TActions>

/**
 * A synchronizable store is a store which has a predefined set of serializable actions. This can be used to replicate the state of the store across multiple processes/hosts.
 *
 * @param initialState - The starting state of the store.
 * @param actions - A record of different actions that can be performed to mutate the state of the store. The parameters must be JSON serializable.
 * @returns
 */
export const createSynchronizableStore = <
  TState,
  TActions extends Record<string, Action<TState>>
>(
  initialState: TState,
  actions: TActions
): SynchronizableStore<TState, TActions> => {
  const signal = createSignal<TState>(initialState)
  const changes = createTopic<Change>()

  const boundActions = bindActions(actions, signal, changes)

  return {
    ...signal,
    [SynchronizableStoreSymbol]: true,
    changes,
    ...boundActions,
  }
}

export const isSynchronizableStore = (
  object: unknown
): object is SynchronizableStore<unknown, Record<string, Action>> =>
  isObject(object) && object[SynchronizableStoreSymbol] === true
