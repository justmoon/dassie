import { bold } from "picocolors"
import { createServer } from "vite"

import { createLogger } from "@dassie/lib-logger"
import type { EffectContext } from "@dassie/lib-reactive"

const logger = createLogger("das:dev:wallet-server")

const walletPath = new URL("../../../../app-wallet", import.meta.url).pathname
const port = 3000

export const serveWallet = async (sig: EffectContext) => {
  const server = await createServer({
    root: walletPath,
    mode: "development",
    server: {
      port,
    },
    define: {
      __DASSIE_NODE_URL__: "'https://n1.localhost:4000'",
    },
  })
  await server.listen()

  console.log(`  ${bold("Wallet UI:")} http://localhost:${port}/\n`)
  logger.info(`listening on http://localhost:${port}/`)

  sig.onCleanup(async () => {
    await server.close()
  })
}
