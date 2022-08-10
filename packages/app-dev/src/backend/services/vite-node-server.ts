import { createValue } from "@dassie/lib-reactive"
import { ViteNodeServer } from "vite-node/server"

import { viteServerValue } from "./vite-server"

export const viteNodeServerValue = () =>
  createValue(async (sig) => {
    const viteServer = await sig.get(viteServerValue)

    // create vite-node server
    const nodeServer = new ViteNodeServer(viteServer)

    return nodeServer
  })
