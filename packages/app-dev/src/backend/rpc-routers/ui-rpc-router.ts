import { createTRPCClient } from "@trpc/client"
import * as trpc from "@trpc/server"
import superjson from "superjson"
import { z } from "zod"

import type { Reactor } from "@xen-ilp/lib-reactive"
import { createRouter as createRemoteReactiveRouter } from "@xen-ilp/lib-reactive-trpc/server"

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
const remoteReactiveRouter = createRemoteReactiveRouter(exposedStores).flat()

export type RemoteReactiveRouter = typeof remoteReactiveRouter

export const uiRpcRouter = trpc
  .router<Reactor>()
  .transformer(superjson)
  .merge(remoteReactiveRouter)
  .query("config", {
    resolve() {
      return config
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

export type UiRpcRouter = typeof uiRpcRouter
