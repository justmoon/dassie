import type { FetchResult, ViteNodeResolveId } from "vite-node"
import { ViteNodeRunner } from "vite-node/client"

// Please note that this file is statically compiled and intentionally minimal. Any additional functionality you're thinking about adding, you should consider adding to the launchers/*.ts instead.

let unique = 0
const callRpc = (method: string, parameters: unknown[]) => {
  const id = unique++
  return new Promise((resolve, reject) => {
    process.send!({
      id,
      method,
      params: parameters,
    })
    const handleResponse = (message: Record<string, unknown>) => {
      if ("id" in message && message["id"] === id) {
        process.removeListener("message", handleResponse)
        if ("result" in message) {
          resolve(message["result"])
        } else {
          reject(message["error"])
        }
      }
    }
    process.on("message", handleResponse)
  })
}

const runner = new ViteNodeRunner({
  root: process.env["DASSIE_DEV_ROOT"]!,
  base: process.env["DASSIE_DEV_BASE"]!,
  async fetchModule(id) {
    return callRpc("fetchModule", [id]) as Promise<FetchResult>
  },
  async resolveId(id, importer) {
    return callRpc("resolveId", [
      id,
      importer,
    ]) as Promise<ViteNodeResolveId | null>
  },
})

// load vite environment
await runner.executeId("/@vite/env")

// execute the file
await runner.executeId(process.env["DASSIE_DEV_ENTRY"]!)
