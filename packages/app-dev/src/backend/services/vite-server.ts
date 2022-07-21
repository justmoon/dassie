import { createServer } from "vite"

import type { Reactor } from "@xen-ilp/lib-reactive"

export const viteServerFactory = async (reactor: Reactor) => {
  // create vite server
  const viteServer = await createServer({
    server: { hmr: false },
    optimizeDeps: {
      // It's recommended to disable deps optimization
      disabled: true,
    },
  })

  // this is needed to initialize the plugins
  await viteServer.pluginContainer.buildStart({})

  reactor.onCleanup(async () => {
    await viteServer.close()
  })

  return viteServer
}
