import { ViteNodeServer } from "vite-node/server"

import { createActor } from "@dassie/lib-reactive"

import { viteService } from "./vite-server"

export const viteNodeService = () =>
  createActor((sig) => {
    const viteServer = sig.get(viteService)

    if (!viteServer) return

    // create vite-node server
    const nodeServer = new ViteNodeServer(viteServer)

    return nodeServer
  })
