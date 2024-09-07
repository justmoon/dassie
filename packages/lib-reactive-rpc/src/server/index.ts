import type {
  Action,
  ActorContext,
  Factory,
  InferChanges,
  Reactor,
  ReadonlySignal,
  ReadonlyTopic,
  Store,
} from "@dassie/lib-reactive"
import { type Subscription, createSubscription } from "@dassie/lib-rpc/server"

export type ReactiveContext = { reactor: Reactor }

export const subscribeToTopic = <TMessage>(
  sig: ActorContext,
  topicFactory: Factory<ReadonlyTopic<TMessage>> | ReadonlyTopic<TMessage>,
): Subscription<TMessage> => {
  const topic =
    typeof topicFactory === "function" ?
      sig.reactor.use(topicFactory)
    : topicFactory

  return createSubscription<TMessage>((onData) => {
    const listener = (message: TMessage) => {
      onData(message)
    }

    topic.on(sig, listener)

    return () => {
      topic.off(listener)
    }
  })
}

export const subscribeToSignal = <TMessage>(
  sig: ActorContext,
  signalFactory: Factory<ReadonlySignal<TMessage>> | ReadonlySignal<TMessage>,
): Subscription<TMessage> => {
  const signal =
    typeof signalFactory === "function" ?
      sig.reactor.use(signalFactory)
    : signalFactory

  return createSubscription<TMessage>((onData) => {
    const listener = (message: TMessage) => {
      onData(message)
    }

    signal.values.on(sig, listener)

    onData(signal.read())

    return () => {
      signal.values.off(listener)
    }
  })
}

export type StoreMessage<
  TState,
  TActions extends Record<string, Action<TState>>,
> =
  | { type: "initial"; value: TState }
  | { type: "changes"; value: InferChanges<TActions> }

export const subscribeToStore = <
  TState,
  TActions extends Record<string, Action<TState>>,
>(
  sig: ActorContext,
  storeFactory: Factory<Store<TState, TActions>> | Store<TState, TActions>,
): Subscription<StoreMessage<TState, TActions>> => {
  const store =
    typeof storeFactory === "function" ?
      sig.reactor.use(storeFactory)
    : storeFactory

  return createSubscription<StoreMessage<TState, TActions>>((onData) => {
    const listener = (message: InferChanges<TActions>) => {
      onData({ type: "changes", value: message })
    }

    store.changes.on(sig, listener)

    onData({ type: "initial", value: store.read() })

    return () => {
      store.changes.off(listener)
    }
  })
}
