import { ViteNodeRunner } from "vite-node/client"

let unique = 0

const callbacks = new Map()

const sendRpc = async (method, params) => {
  return new Promise((resolve, reject) => {
    const id = String(unique++)
    process.send({ id, method, params })
    callbacks.set(id, { resolve, reject })
  })
}

process.on("message", async (msg) => {
  if (msg.method === "start") {
    try {
      // create vite-node runner
      const runner = new ViteNodeRunner({
        root: msg.params.root,
        base: msg.params.base,
        // when having the server and runner in a different context,
        // you will need to handle the communication between them
        // and pass to this function
        async fetchModule(id) {
          return sendRpc("fetchModule", [id])
        },
        async resolveId(id, importer) {
          return sendRpc("resolveId", [id, importer])
        },
      })

      // load vite environment
      await runner.executeId("/@vite/env")

      // execute the file
      const startApp = (await runner.executeFile(msg.params.entry)).default
      await startApp(msg.params.config)
    } catch (error) {
      console.error("error while starting up:", error)
    }
  } else if (msg.method === "exit") {
    process.exit(0)
  } else if (msg.result || msg.error) {
    if (callbacks.has(msg.id)) {
      const { resolve, reject } = callbacks.get(msg.id)
      if (msg.error) {
        reject(new Error(msg.error.message))
      } else {
        resolve(msg.result)
      }
      callbacks.delete(msg.id)
    } else {
      console.error(`Unknown RPC message: ${msg}`)
    }
  }
})
