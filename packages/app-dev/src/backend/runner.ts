#!/usr/bin/env node
import { ViteNodeRunner } from "vite-node/client"

import { assertDefined } from "@xen-ilp/lib-type-utils"

import RpcHost from "../classes/rpc-host"
import { schema } from "../schemas/server-request"
import type { FetchResult, ViteNodeResolveId } from "vite-node"

const sendMessage = (message: unknown) => {
  if (!process.send) {
    throw new Error("process.send is not defined")
  }
  process.send(message)
}

const rpcClient = new RpcHost(schema, () => Promise.resolve(null), sendMessage)
process.on("message", (message) => void rpcClient.handleMessage(message))

assertDefined(process.env["XEN_DEV_ROOT"])
assertDefined(process.env["XEN_DEV_BASE"])

const runner = new ViteNodeRunner({
  root: process.env["XEN_DEV_ROOT"],
  base: process.env["XEN_DEV_BASE"],
  async fetchModule(id) {
  return (await rpcClient.call("fetchModule", [id])) as FetchResult
},
async resolveId(id, importer) {
  return (await rpcClient.call("resolveId", [
    id,
    importer,
  ])) as ViteNodeResolveId | null
}
})

// load vite environment
await runner.executeId("/@vite/env")

// execute the file
assertDefined(process.env["XEN_DEV_ENTRY"])
await runner.executeId(process.env["XEN_DEV_ENTRY"])
