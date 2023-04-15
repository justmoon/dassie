import { isObject } from "@dassie/lib-type-utils"

import { type Signal, createSignal } from "./signal"
import { type ReadonlyTopic, type Topic, createTopic } from "./topic"

export const StoreSymbol = Symbol("das:reactive:store")

export type Action<
  TState = unknown,
  TParameters extends unknown[] = never[]
> = (...parameters: TParameters) => (previousState: TState) => TState

export type Change = [actionId: string, parameters: unknown[]]

export type InferBoundAction<TAction> = TAction extends Action<
  infer S,
  infer P extends unknown[]
>
  ? BoundAction<S, P>
  : never

export type InferBoundActions<TActions> = {
  [key in keyof TActions]: InferBoundAction<TActions[key]>
}

export type InferChanges<TActions> = {
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  [key in keyof TActions]: TActions[key] extends Action<any, any[]>
    ? [key, Parameters<TActions[key]>]
    : never
}[keyof TActions]

export type BoundAction<TState, TParameters extends unknown[]> = (
  this: void,
  ...parameters: TParameters
) => TState

const bindActions = <TState, TActions extends Record<string, Action<TState>>>(
  actions: TActions,
  signal: Signal<TState>,
  changesTopic: Topic<InferChanges<TActions>>
): InferBoundActions<TActions> => {
  const boundActions = {} as InferBoundActions<TActions>

  for (const key in actions) {
    if (Object.prototype.hasOwnProperty.call(actions, key)) {
      boundActions[key] = ((...parameters) => {
        const reducer = actions[key]!(...parameters)

        signal.update(reducer)
        changesTopic.emit([
          key,
          parameters,
        ] as unknown as InferChanges<TActions>)

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

  changes: ReadonlyTopic<InferChanges<TActions>>
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
  const changes = createTopic<InferChanges<TActions>>()

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
