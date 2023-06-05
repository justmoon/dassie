import { type AnyRootConfig, type RootConfig, initTRPC } from "@trpc/server"
import { type Observable, observable } from "@trpc/server/observable"

import {
  ActorContext,
  type Change,
  type Factory,
  type Reactor,
  ReadonlySignal,
  type ReadonlyTopic,
  isSignal,
  isStore,
} from "@dassie/lib-reactive"

// eslint-disable-next-line @typescript-eslint/consistent-type-definitions
export type ReactiveContext = { reactor: Reactor }

const _trpc = initTRPC
  .context<ReactiveContext>()
  // eslint-disable-next-line @typescript-eslint/no-unsafe-assignment, @typescript-eslint/no-explicit-any
  .create({ transformer: {} as any })

type AnyRootConfigTypes = AnyRootConfig extends RootConfig<infer T> ? T : never

export interface ReactiveCapableTrpc {
  _config: RootConfig<AnyRootConfigTypes & { ctx: ReactiveContext }>
}

export type ExposedTopicsMap = Record<string, Factory<ReadonlyTopic<unknown>>>

export const subscribeToSignal = <TValue>(
  sig: ActorContext,
  signalFactory: Factory<ReadonlySignal<TValue>>
): Observable<TValue, unknown> => {
  return observable<TValue>((emit) => {
    const signal = sig.use(signalFactory)
    const listener = (value: TValue) => {
      emit.next(value)
    }
    signal.on(sig, listener)
    listener(signal.read())
    return () => {
      signal.off(listener)
    }
  })
}

export const createRemoteReactiveRouter = <
  TExposedTopicsMap extends ExposedTopicsMap
>(
  trpc: ReactiveCapableTrpc,
  exposedTopics: TExposedTopicsMap
) => {
  const validateTopicName = (topicName: unknown): keyof TExposedTopicsMap => {
    if (typeof topicName === "string" && topicName in exposedTopics) {
      return topicName as keyof TExposedTopicsMap
    }

    throw new Error("Invalid store name")
  }

  const castTrpc = trpc as typeof _trpc

  const router = castTrpc.router({
    exposedTopics: castTrpc.procedure.query(
      () => null as unknown as TExposedTopicsMap
    ),
    getSignalState: castTrpc.procedure
      .input(validateTopicName)
      .query(({ input: signalName, ctx: { reactor } }) => {
        const topicFactory =
          exposedTopics[signalName as keyof TExposedTopicsMap]

        if (!topicFactory) {
          throw new Error("Invalid store name")
        }

        const signal = reactor.use(topicFactory)

        if (!isSignal(signal)) {
          throw new Error(`Topic is not a store`)
        }

        return {
          value: signal.read(),
        }
      }),
    listenToTopic: castTrpc.procedure
      .input(validateTopicName)
      .subscription(({ input: topicName, ctx: { reactor } }) => {
        return observable<unknown>((emit) => {
          const topicFactory =
            exposedTopics[topicName as keyof TExposedTopicsMap]
          if (!topicFactory) {
            throw new Error("Invalid topic name")
          }
          const store = reactor.use(topicFactory)

          if (isSignal(store)) {
            emit.next(store.read())
          }

          const listener = (value: unknown) => {
            emit.next(value)
          }
          store.on(reactor, listener)
          return () => store.off(listener)
        })
      }),
    listenToChanges: castTrpc.procedure
      .input(validateTopicName)
      .subscription(({ input: storeName, ctx: { reactor } }) => {
        return observable<
          | {
              type: "initial"
              data: unknown
            }
          | {
              type: "change"
              data: Change
            }
        >((emit) => {
          const topicFactory =
            exposedTopics[storeName as keyof TExposedTopicsMap]
          if (!topicFactory) {
            throw new Error("Invalid topic name")
          }

          const store = reactor.use(topicFactory)

          if (!isStore(store)) {
            throw new Error("Target is not a store")
          }

          emit.next({
            type: "initial",
            data: store.read(),
          })

          const listener = (change: Change) => {
            emit.next({
              type: "change",
              data: change,
            })
          }

          store.changes.on(reactor, listener)

          return () => {
            store.changes.off(listener)
          }
        })
      }),
  })

  return router
}

export type RemoteReactiveRouter<TExposedTopicsMap extends ExposedTopicsMap> =
  ReturnType<typeof createRemoteReactiveRouter<TExposedTopicsMap>>
