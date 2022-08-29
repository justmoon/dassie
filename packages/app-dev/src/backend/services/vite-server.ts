import { createServer } from "vite"

import { createService } from "@dassie/lib-reactive"

export const viteService = () =>
  createService(async (sig) => {
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

    sig.onCleanup(async () => {
      await viteServer.close()
    })

    return viteServer
  })
