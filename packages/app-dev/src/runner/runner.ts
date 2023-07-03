import type { FetchResult, ViteNodeResolveId } from "vite-node"
import { ModuleCacheMap, ViteNodeRunner } from "vite-node/client"
import { installSourcemapsSupport } from "vite-node/source-map"

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

const moduleCache = new ModuleCacheMap()

// fixes stacktraces in Errors
installSourcemapsSupport({
  getSourceMap: (source) => moduleCache.getSourceMap(source),
})

const runner = new ViteNodeRunner({
  root: process.env["DASSIE_DEV_ROOT"]!,
  base: process.env["DASSIE_DEV_BASE"]!,
  moduleCache,
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

// indicate readiness
await callRpc("ready", [])
