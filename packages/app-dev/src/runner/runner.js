/**
 * @typedef {import("vite-node").FetchResult} FetchResult
 * @typedef {import("vite-node").ViteNodeResolveId} ViteNodeResolveId
 */
import { ModuleCacheMap, ViteNodeRunner } from "vite-node/client"
import { installSourcemapsSupport } from "vite-node/source-map"

// Please note that this file is intentionally minimal. Any additional functionality you're thinking about adding, you should consider adding to the launchers/*.ts instead.

const { DASSIE_DEV_ROOT, DASSIE_DEV_BASE, DASSIE_DEV_ENTRY } = process.env

if (!DASSIE_DEV_ROOT) {
  throw new Error("Missing required environment variable: DASSIE_DEV_ROOT")
}

if (!DASSIE_DEV_BASE) {
  throw new Error("Missing required environment variable: DASSIE_DEV_BASE")
}

if (!DASSIE_DEV_ENTRY) {
  throw new Error("Missing required environment variable: DASSIE_DEV_ENTRY")
}

// eslint-disable-next-line @dassie/no-top-level-mutables
let unique = 0
/**
 * @param {string} method - RPC method name
 * @param {unknown[]} parameters - RPC method parameters
 * @returns {Promise<unknown>}
 */
const callRpc = (method, parameters) => {
  const id = unique++
  return new Promise((resolve, reject) => {
    if (!process.send) {
      throw new Error("process.send is not defined")
    }

    process.send({
      id,
      method,
      params: parameters,
    })
    /**
     * @param {Record<string, unknown>} message - RPC response
     */
    const handleResponse = (message) => {
      if ("id" in message && message["id"] === id) {
        process.removeListener("message", handleResponse)
        if ("result" in message) {
          resolve(message["result"])
        } else {
          reject(new Error(/** @type {string} */ (message["error"])))
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
  root: DASSIE_DEV_ROOT,
  base: DASSIE_DEV_BASE,
  moduleCache,
  async fetchModule(id) {
    return /** @type {Promise<FetchResult>} */ (callRpc("fetchModule", [id]))
  },
  async resolveId(id, importer) {
    return /** @type {Promise<ViteNodeResolveId | null>} */ (
      callRpc("resolveId", [id, importer])
    )
  },
})

// load vite environment
await runner.executeId("/@vite/env")

// execute the file
await runner.executeId(DASSIE_DEV_ENTRY)

// indicate readiness
await callRpc("ready", [])
