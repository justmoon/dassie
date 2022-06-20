#!/usr/bin/env node
import type { FetchResult, ViteNodeResolveId } from "vite-node"
import { ViteNodeRunner } from "vite-node/client"

import RpcHost from "./classes/rpc-host"
import { ServerRequest, schema } from "./schemas/server-request"

const sendMessage = (message: unknown) => {
  if (!process.send) {
    throw new Error("process.send is not defined")
  }
  process.send(message)
}

const handleRequest = async (request: ServerRequest) => {
  switch (request.method) {
    case "start": {
      // create vite-node runner
      const runner = new ViteNodeRunner({
        root: request.params.root,
        base: request.params.base,
        // when having the server and runner in a different context,
        // you will need to handle the communication between them
        // and pass to this function
        async fetchModule(id) {
          return (await rpcClient.call("fetchModule", [id])) as FetchResult
        },
        async resolveId(id, importer) {
          return (await rpcClient.call("resolveId", [
            id,
            importer,
          ])) as ViteNodeResolveId | null
        },
      })

      // load vite environment
      await runner.executeId("/@vite/env")

      // execute the file
      const { default: startApp } = await runner.executeId(request.params.entry)
      await startApp(request.params.config)
      return
    }
    case "exit":
      return process.exit(0)
  }
}

const rpcClient = new RpcHost(schema, handleRequest, sendMessage)
process.on("message", rpcClient.handleMessage.bind(rpcClient))
