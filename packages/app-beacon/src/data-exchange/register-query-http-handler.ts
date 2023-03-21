import { respondJson } from "@dassie/lib-http-server"
import { createActor } from "@dassie/lib-reactive"

import { routerService } from "../http-server/serve-http"
import { TrackedNode, trackedNodes } from "./track-nodes"

export interface QueryResponseData {
  nodes: TrackedNode[]
}

export const registerQueryHttpHandler = () =>
  createActor((sig) => {
    const router = sig.get(routerService)

    if (!router) return

    router.get("/query", (request, response) => {
      response.setHeader("Access-Control-Allow-Origin", "*")
      const query = new URLSearchParams(
        new URL(request.url!, "http://localhost/").search
      )

      const subnet = query.get("subnet")

      if (!subnet) {
        respondJson(response, 400, {
          error: "Bad Request",
          message: "Missing subnet query parameter",
        })
        return
      }

      const subnetNodes = trackedNodes.get(subnet)

      const nodes = subnetNodes ? [...subnetNodes.values()] : []

      const data: QueryResponseData = {
        nodes,
      }

      respondJson(response, 200, data)
    })
  })
