import { isObject } from "@dassie/lib-type-utils"

import { createReactiveTopic } from "./internal/reactive-topic"
import type { Scope } from "./scope"
import { type Signal, SignalImplementation } from "./signal"
import { type ReadonlyTopic, type Topic, createTopic } from "./topic"
import type { FactoryOrInstance } from "./types/factory"
import type { ScopeContext } from "./types/scope-context"

export const StoreSymbol = Symbol("das:reactive:store")

export type Action<
  TState = unknown,
  TParameters extends unknown[] = never[],
> = (...parameters: TParameters) => (previousState: TState) => TState

export type Change = [actionId: string, parameters: unknown[]]

export type InferBoundAction<TAction> =
  TAction extends Action<infer S, infer P extends unknown[]> ? BoundAction<S, P>
  : never

export type InferBoundActions<TActions> = {
  [key in keyof TActions]: InferBoundAction<TActions[key]>
}

export type InferChanges<TActions> = {
  [key in keyof TActions & string]: TActions[key] extends (
    Action<infer _TState, infer TParameters extends unknown[]>
  ) ?
    [key, TParameters]
  : never
}[keyof TActions & string]

export type InferActionHandlers<TStore extends FactoryOrInstance<AnyStore>> =
  TStore extends Store<infer TState, infer TActions> ?
    {
      [K in keyof TActions]: TActions[K] extends (
        Action<TState, infer TParameters>
      ) ?
        (...parameters: TParameters) => void
      : never
    }
  : never

export type BoundAction<TState, TParameters extends unknown[]> = (
  this: void,
  ...parameters: TParameters
) => TState

const bindActions = <TState, TActions extends Record<string, Action<TState>>>(
  actions: TActions,
  signal: Signal<TState>,
  changesTopic: Topic<InferChanges<TActions>>,
): InferBoundActions<TActions> => {
  const boundActions = {} as InferBoundActions<TActions>

  for (const key in actions) {
    if (Object.prototype.hasOwnProperty.call(actions, key)) {
      boundActions[key] = ((
        ...parameters: Parameters<TActions[typeof key]>
      ) => {
        const reducer = actions[key]!(...parameters)

        signal.update(reducer)
        changesTopic.emit([
          key,
          parameters,
        ] as unknown as InferChanges<TActions>)

        return signal.read()
      }) as InferBoundActions<TActions>[typeof key]
    }
  }

  return boundActions
}

export type Store<
  TState,
  TActions extends Record<string, Action<TState>>,
> = Signal<TState> & {
  /**
   * Marks this object as a store.
   */
  [StoreSymbol]: true

  /**
   * Object containing bound actions that can be invoked to mutate the state of the store.
   */
  act: InferBoundActions<TActions>

  /**
   * A topic that will emit a change object every time a bound action is invoked.
   *
   * The change object is a tuple containing the name of the action and the parameters passed to the action.
   */
  changes: ReadonlyTopic<InferChanges<TActions>>

  /**
   * Create a new store which supports additional actions.
   *
   * @remarks
   *
   * The new store's initial state will be the same as the current store's current state.
   * @param additionalActions - An object where the keys are the names of the actions and the values are the action functions.
   * @returns The modified store supporting the additional actions.
   */
  actions: <TAdditionalActions extends Record<string, Action<TState>>>(
    additionalActions: TAdditionalActions,
  ) => Store<TState, TActions & TAdditionalActions>
}

// eslint-disable-next-line @typescript-eslint/no-explicit-any
export type AnyStore = Store<any, Record<string, Action<any, any[]>>>

class StoreImplementation<
  TState,
  TActions extends Record<string, Action<TState>>,
> extends SignalImplementation<TState> {
  [StoreSymbol] = true as const

  changes = createTopic<InferChanges<TActions>>()

  act: InferBoundActions<TActions>

  constructor(
    initialState: TState,
    private readonly rawActions: TActions,
  ) {
    super(initialState)

    this.act = bindActions(rawActions, this, this.changes)
  }

  actions<TAdditionalActions extends Record<string, Action<TState>>>(
    additionalActions: TAdditionalActions,
  ): Store<TState, TActions & TAdditionalActions> {
    return new StoreImplementation(
      this.cache,
      Object.assign({}, this.rawActions, additionalActions),
    )
  }
}

/**
 * A store is a signal which has a predefined set of serializable actions. This can be used to replicate the state of the store across multiple processes/hosts.
 *
 * @param initialState - The starting state of the store.
 * @param actions - A record of different actions that can be performed to mutate the state of the store. The parameters must be JSON serializable.
 * @returns
 */
export const createStore = <TState>(
  initialState: TState,
): Store<TState, {}> => {
  const store = new StoreImplementation(initialState, {})

  return Object.assign(store, createReactiveTopic(store))
}

export const isStore = (
  object: unknown,
): object is Store<unknown, Record<string, Action>> =>
  isObject(object) && object[StoreSymbol] === true

export const watchStoreChanges = <TStore extends AnyStore>(
  scope: ScopeContext | Scope | undefined,
  store: TStore,
  handlers: InferActionHandlers<TStore>,
) => {
  store.changes.on(scope, ([actionId, parameters]) => {
    // eslint-disable-next-line @typescript-eslint/no-unsafe-argument
    handlers[actionId]?.(...parameters)
  })
}
