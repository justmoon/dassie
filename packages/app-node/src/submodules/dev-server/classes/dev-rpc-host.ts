import type { ViteNodeServer } from "vite-node/server"
import z from "zod"

export default class DevRpcHost {
  constructor(private nodeServer: ViteNodeServer) {}

  private readonly schema = z.discriminatedUnion("method", [
    z.object({
      method: z.literal("fetchModule"),
      params: z.tuple([z.string()]),
    }),
    z.object({
      method: z.literal("resolveId"),
      params: z.tuple([z.string(), z.union([z.string(), z.null()])]),
    }),
  ])

  call(rawRequest: unknown) {
    const request = this.schema.parse(rawRequest)

    switch (request.method) {
      case "fetchModule":
        return this.nodeServer.fetchModule(...request.params)
      case "resolveId":
        return this.nodeServer.resolveId(
          request.params[0],
          request.params[1] === null ? undefined : request.params[1]
        )
    }
  }

  fetchModule(id: string) {
    return this.nodeServer.fetchModule(id)
  }

  resolveId(id: string, importer: string) {
    return this.nodeServer.resolveId(id, importer)
  }
}
