import { isObject } from "@dassie/lib-type-utils"

import type { Effect } from "./effect"
import { Signal, createSignal } from "./signal"
import { Topic, createTopic } from "./topic"

export const StoreSymbol = Symbol("das:reactive:store")

export type StoreFactory<
  TState = unknown,
  TActions extends Record<string, Action<TState>> = Record<
    string,
    Action<TState>
  >
> = Effect<undefined, Store<TState, TActions>>

export type Action<
  TState = unknown,
  TParameters extends unknown[] = never[]
> = (...parameters: TParameters) => (previousState: TState) => TState

export type Change = [string, unknown[]]

export type InferBoundAction<TAction> = TAction extends Action<
  infer S,
  infer P extends unknown[]
>
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
  signal: Signal<TState>,
  changesTopic: Topic<Change, Change>
): InferBoundActions<TActions> => {
  const boundActions = {} as InferBoundActions<TActions>

  for (const key in actions) {
    if (Object.prototype.hasOwnProperty.call(actions, key)) {
      boundActions[key] = ((...parameters) => {
        const reducer = actions[key]!(...parameters)

        signal.update(reducer)
        changesTopic.emit([key, parameters])

        return signal.read()
      }) as (typeof boundActions)[typeof key]
    }
  }

  return boundActions
}

export type Store<
  TState,
  TActions extends Record<string, Action<TState>>
> = Signal<TState> & {
  /**
   * Marks this object as a store.
   */
  [StoreSymbol]: true

  changes: Topic<Change>
} & InferBoundActions<TActions>

/**
 * A store is a signal which has a predefined set of serializable actions. This can be used to replicate the state of the store across multiple processes/hosts.
 *
 * @param initialState - The starting state of the store.
 * @param actions - A record of different actions that can be performed to mutate the state of the store. The parameters must be JSON serializable.
 * @returns
 */
export const createStore = <
  TState,
  TActions extends Record<string, Action<TState>>
>(
  initialState: TState,
  actions: TActions
): Store<TState, TActions> => {
  const signal = createSignal<TState>(initialState)
  const changes = createTopic<Change>()

  const boundActions = bindActions(actions, signal, changes)

  return {
    ...signal,
    [StoreSymbol]: true,
    changes,
    ...boundActions,
  }
}

export const isStore = (
  object: unknown
): object is Store<unknown, Record<string, Action>> =>
  isObject(object) && object[StoreSymbol] === true
