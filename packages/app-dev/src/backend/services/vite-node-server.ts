import { ViteNodeServer } from "vite-node/server"

import type { Reactor } from "@xen-ilp/lib-reactive"

import { viteServerFactory } from "./vite-server"

export const viteNodeServerFactory = async (reactor: Reactor) => {
  const viteServer = await reactor.fromContext(viteServerFactory)

  // create vite-node server
  const nodeServer = new ViteNodeServer(viteServer)

  return nodeServer
}
