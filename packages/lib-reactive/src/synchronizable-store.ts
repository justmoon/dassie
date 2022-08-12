
import { isObject } from "@dassie/lib-type-utils"

import { Factory, InitSymbol, Reactor } from "./reactor"
import { Reducer, Store, createStore } from "./store"
import { TopicFactory, createTopic } from "./topic"

export const SynchronizableStoreSymbol = Symbol(
  "das:reactive:synchronizable-store"
)

export interface SynchronizableStoreFactory<
  TState = unknown,
  TActions extends Record<string, Action<TState>> = Record<string, Action<TState>>
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
  store: Store<TState>,
  changesTopic: TopicFactory<Change, Change>,
  reactor: Reactor
): InferBoundActions<TActions> => {
  const boundActions = {} as InferBoundActions<TActions>

  for (const key in actions) {
    if (Object.prototype.hasOwnProperty.call(actions, key)) {
      boundActions[key] = ((...parameters) => {
        const reducer = actions[key]!(...parameters)

        store.emit(reducer)
        reactor.useContext(changesTopic).emit([key, parameters])

        return store.read()
      }) as typeof boundActions[typeof key]
    }
  }

  return boundActions
}

export type SynchronizableStore<
  TState,
  TActions extends Record<string, Action<TState>>
> = Store<TState, Reducer<TState>> & {
  /**
   * Marks this object as a store.
   */
  [SynchronizableStoreSymbol]: true

  [InitSymbol]: (reactor: Reactor) => void

  changes: TopicFactory<Change>
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
  let reactor!: Reactor
  const store = createStore<TState>(initialState)
  // eslint-disable-next-line unicorn/consistent-function-scoping
  const changes = () => createTopic<Change>()

  const boundActions = bindActions(actions, store, changes, reactor)

  return {
    ...store,
    [SynchronizableStoreSymbol]: true,
    [InitSymbol]: (_reactor: Reactor) => {
      reactor = _reactor
    },
    changes,
    ...boundActions,
  }
}

export const isSynchronizableStore = (
  object: unknown
): object is SynchronizableStore<never, never> =>
  isObject(object) && object[SynchronizableStoreSymbol] === true
