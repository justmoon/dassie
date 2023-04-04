import { RootConfig, initTRPC } from "@trpc/server"
import type { AnyRootConfig } from "@trpc/server/dist/core/internals/config"
import { observable } from "@trpc/server/observable"

import {
  Change,
  Factory,
  Reactor,
  ReadonlyTopic,
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

          return store.on((value) => {
            emit.next(value)
          })
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

          const disposeSubscription = store.changes.on((change) => {
            emit.next({
              type: "change",
              data: change,
            })
          })

          return () => {
            disposeSubscription()
          }
        })
      }),
  })

  return router
}

export type RemoteReactiveRouter<TExposedTopicsMap extends ExposedTopicsMap> =
  ReturnType<typeof createRemoteReactiveRouter<TExposedTopicsMap>>
