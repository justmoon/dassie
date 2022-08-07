import { createTRPCClient } from "@trpc/client"
import * as trpc from "@trpc/server"
import superjson from "superjson"
import { z } from "zod"

import type {
  InferMessageType,
  Reactor,
  StoreFactory,
} from "@xen-ilp/lib-reactive"

import type { DebugRpcRouter } from "../../runner/effects/debug-rpc-server"
import { config } from "../config"
import { IndexedLogLine, indexedLogLineTopic } from "../features/logs"
import { activeTemplate } from "../stores/active-template"
import { logsStore } from "../stores/logs"
import {
  GlobalFirehoseMessage,
  globalFirehoseTopic,
} from "../topics/global-firehose"
import { PeerMessageMetadata, peerTrafficTopic } from "../topics/peer-traffic"
import { activeNodeConfig } from "../values/active-node-config"

export const exposedStores = {
  activeNodeConfig,
} as const

export type ExposedStoresMap = typeof exposedStores

const validateStoreName = (storeName: unknown): keyof ExposedStoresMap => {
  if (typeof storeName === "string" && storeName in exposedStores) {
    return storeName as keyof ExposedStoresMap
  }

  throw new Error("Invalid store name")
}

export const uiRpcRouter = trpc
  .router<Reactor>()
  .transformer(superjson)
  .query("config", {
    resolve() {
      return config
    },
  })
  .query("getState", {
    input: validateStoreName,
    resolve({ input: storeName, ctx: reactor }) {
      const store = exposedStores[storeName]

      return {
        value: reactor.useContext(store as StoreFactory<unknown>).read(),
      }
    },
  })
  .query("getNodeState", {
    input: z.tuple([z.string(), z.string()]),
    resolve({ input: [nodeId, storeName], ctx: reactor }) {
      const node = reactor
        .useContext(activeNodeConfig)
        .read()
        .find(({ id }) => id === nodeId)

      if (!node) {
        throw new Error(`Unknown node ID ${nodeId}`)
      }

      const client = createTRPCClient<DebugRpcRouter>({
        url: `http://localhost:${node.debugPort}/trpc`,
        transformer: superjson,
      })

      return client.query("getState", storeName)
    },
  })
  .query("getMessage", {
    input: z.tuple([z.string(), z.number()]),
    async resolve({ input: [nodeId, messageId], ctx: reactor }) {
      const node = reactor
        .useContext(activeNodeConfig)
        .read()
        .find(({ id }) => id === nodeId)

      if (!node) {
        throw new Error(`Unknown node ID ${nodeId}`)
      }

      const client = createTRPCClient<DebugRpcRouter>({
        url: `http://localhost:${node.debugPort}/trpc`,
        transformer: superjson,
      })

      return client.query("getMessage", [messageId])
    },
  })
  .mutation("addRandomNode", {
    resolve({ ctx: reactor }) {
      const templateStore = reactor.useContext(activeTemplate)

      const template = templateStore.read()

      const nodeCount = new Set(template.flat()).size

      const newNodeId = nodeCount

      const peers = Array.from({ length: Math.min(nodeCount, 3) })
        .fill(undefined)
        .map(() => Math.floor(Math.random() * nodeCount))

      const uniquePeers = [...new Set(peers)]

      const newLinks = uniquePeers.map((peer) => [newNodeId, peer] as const)

      templateStore.emit((links) => [...links, ...newLinks])
    },
  })
  .subscription("logs", {
    resolve({ ctx: { useContext: fromContext } }) {
      return new trpc.Subscription<IndexedLogLine>((sendToClient) => {
        const previousLogs = fromContext(logsStore).read()

        for (const logLine of previousLogs) {
          sendToClient.data(logLine)
        }

        return fromContext(indexedLogLineTopic).on((logLine) => {
          sendToClient.data(logLine)
        })
      })
    },
  })
  .subscription("globalFirehose", {
    resolve({ ctx: { useContext: fromContext } }) {
      return new trpc.Subscription<GlobalFirehoseMessage>((sendToClient) => {
        return fromContext(globalFirehoseTopic).on((logLine) => {
          sendToClient.data(logLine)
        })
      })
    },
  })
  .subscription("peerTraffic", {
    resolve({ ctx: { useContext: fromContext } }) {
      return new trpc.Subscription<PeerMessageMetadata>((sendToClient) => {
        return fromContext(peerTrafficTopic).on((message) => {
          sendToClient.data(message)
        })
      })
    },
  })
  .subscription("getLiveState", {
    input: validateStoreName,
    resolve: <T extends keyof ExposedStoresMap>({
      input: storeName,
      ctx: reactor,
    }: {
      input: T
      ctx: Reactor
    }) => {
      return new trpc.Subscription<InferMessageType<ExposedStoresMap[T]>>(
        (sendToClient) => {
          const store = reactor.useContext(exposedStores[storeName])

          sendToClient.data(
            store.read() as InferMessageType<ExposedStoresMap[T]>
          )

          return store.on((value) => {
            sendToClient.data(value as InferMessageType<ExposedStoresMap[T]>)
          })
        }
      )
    },
  })

export type UiRpcRouter = typeof uiRpcRouter
