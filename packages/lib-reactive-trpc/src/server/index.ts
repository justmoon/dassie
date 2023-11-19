import { type Observable, observable } from "@trpc/server/observable"

import {
  Action,
  ActorContext,
  type Factory,
  InferChanges,
  type Reactor,
  ReadonlySignal,
  type ReadonlyTopic,
  Store,
} from "@dassie/lib-reactive"

// eslint-disable-next-line @typescript-eslint/consistent-type-definitions
export type ReactiveContext = { reactor: Reactor }

export interface TopicSubscriptionOptions<TMessage> {
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  transform?: (item: TMessage) => any
}

type InferEmittedValue<TMessage, TOptions> = TOptions extends {
  transform: (item: TMessage) => infer TOutput
}
  ? TOutput
  : TMessage

export const subscribeToTopic = <
  TMessage,
  TOptions extends TopicSubscriptionOptions<TMessage> | undefined,
>(
  sig: ActorContext,
  topicFactory: Factory<ReadonlyTopic<TMessage>> | ReadonlyTopic<TMessage>,
  options?: TOptions,
): Observable<InferEmittedValue<TMessage, TOptions>, unknown> => {
  const topic =
    typeof topicFactory === "function"
      ? sig.reactor.use(topicFactory)
      : topicFactory

  return observable<TMessage>((emit) => {
    const listener = (message: TMessage) => {
      // eslint-disable-next-line @typescript-eslint/no-unsafe-argument
      emit.next(options?.transform ? options.transform(message) : message)
    }
    topic.on(sig, listener)
    return () => {
      topic.off(listener)
    }
  })
}

export interface SignalSubscriptionOptions {
  batching?: boolean
}

export const subscribeToSignal = <TValue>(
  sig: ActorContext,
  signalFactory: Factory<ReadonlySignal<TValue>> | ReadonlySignal<TValue>,
  { batching = true }: SignalSubscriptionOptions = {},
): Observable<TValue, unknown> => {
  const signal =
    typeof signalFactory === "function"
      ? sig.reactor.use(signalFactory)
      : signalFactory

  return observable<TValue>((emit) => {
    let timer: ReturnType<typeof setImmediate> | undefined
    const listener = (value: TValue) => {
      if (batching) {
        if (!timer) {
          timer = setImmediate(() => {
            emit.next(value)
            timer = undefined
          })
        }
        return
      }

      emit.next(value)
    }
    signal.on(sig, listener)
    listener(signal.read())
    return () => {
      signal.off(listener)
    }
  })
}

export type StoreMessage<
  TState,
  TActions extends Record<string, Action<TState>>,
> =
  | { type: "initial"; value: TState }
  | { type: "changes"; value: readonly InferChanges<TActions>[] }

export const subscribeToStore = <
  TState,
  TActions extends Record<string, Action<TState>>,
>(
  sig: ActorContext,
  storeFactory: Factory<Store<TState, TActions>> | Store<TState, TActions>,
  { batching = true }: SignalSubscriptionOptions = {},
): Observable<StoreMessage<TState, TActions>, never> => {
  const store =
    typeof storeFactory === "function"
      ? sig.reactor.use(storeFactory)
      : storeFactory

  return observable<StoreMessage<TState, TActions>, never>((emit) => {
    let timer: ReturnType<typeof setImmediate> | undefined
    const queuedChanges = new Set<InferChanges<TActions>>()

    const listener = (change: InferChanges<TActions>) => {
      if (batching) {
        if (!timer) {
          timer = setImmediate(() => {
            emit.next({
              type: "changes",
              value: [...queuedChanges],
            })
            queuedChanges.clear()
            timer = undefined
          })
        }

        queuedChanges.add(change)
        return
      }

      emit.next({
        type: "changes",
        value: [change],
      })
    }
    store.changes.on(sig, listener)

    emit.next({ type: "initial", value: store.read() })
  })
}
