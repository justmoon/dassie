import { createServer } from "vite"

import { createLogger } from "@xen-ilp/lib-logger"

const logger = createLogger("xen:dev:wallet-server")

const walletPath = new URL("../../app-wallet", import.meta.url).pathname
const port = 3000

export const startWalletServer = async () => {
  const server = await createServer({
    // any valid user config options, plus `mode` and `configFile`
    root: walletPath,
    server: {
      port,
    },
  })
  await server.listen()

  logger.info(`listening on http://localhost:${port}/`)
}
