#!/usr/bin/env node
import { createTRPCClient } from "@trpc/client"
import { createWSClient, wsLink } from "@trpc/client/links/wsLink"
import { ViteNodeRunner } from "vite-node/client"
import WebSocket from "ws"

import { assertDefined } from "@xen-ilp/lib-type-utils"

import type { AppRouter } from "../backend/rpc-routers/app-router"

const wsClient = createWSClient({
  url: process.env["XEN_DEV_RPC_URL"]!,
  // eslint-disable-next-line @typescript-eslint/no-unsafe-assignment, @typescript-eslint/no-explicit-any
  WebSocket: WebSocket as any,
})

const trpcClient = createTRPCClient<AppRouter>({
  links: [
    wsLink({
      client: wsClient,
    }),
  ],
})

assertDefined(process.env["XEN_DEV_ROOT"])
assertDefined(process.env["XEN_DEV_BASE"])

const runner = new ViteNodeRunner({
  root: process.env["XEN_DEV_ROOT"],
  base: process.env["XEN_DEV_BASE"],
  async fetchModule(id) {
    return await trpcClient.query("runner.fetchModule", [id])
  },
  async resolveId(id, importer) {
    return await trpcClient.query("runner.resolveId", [id, importer])
  },
})

// send message to indicate that we're up and running
process.send?.({})

// load vite environment
await runner.executeId("/@vite/env")

// execute the file
assertDefined(process.env["XEN_DEV_ENTRY"])
await runner.executeId(process.env["XEN_DEV_ENTRY"])
