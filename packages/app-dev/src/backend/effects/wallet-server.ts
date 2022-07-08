import { bold } from "picocolors"
import { createServer } from "vite"

import { createLogger } from "@xen-ilp/lib-logger"
import type { EffectContext } from "@xen-ilp/lib-reactive"

const logger = createLogger("xen:dev:wallet-server")

const walletPath = new URL("../../../../app-wallet", import.meta.url).pathname
const port = 3000

export const walletServer = async (sig: EffectContext) => {
  const server = await createServer({
    root: walletPath,
    server: {
      port,
    },
  })
  await server.listen()

  console.log(`  ${bold("Wallet UI:")} http://localhost:${port}/\n`)
  logger.info(`listening on http://localhost:${port}/`)

  sig.onCleanup(async () => {
    await server.close()
  })
}
