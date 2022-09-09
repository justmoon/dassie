import { ViteNodeServer } from "vite-node/server"

import { createService } from "@dassie/lib-reactive"

import { viteService } from "./vite-server"

export const viteNodeService = () =>
  createService(async (sig) => {
    const viteServer = await sig.get(viteService)

    if (!viteServer) return

    // create vite-node server
    const nodeServer = new ViteNodeServer(viteServer)

    return nodeServer
  })
