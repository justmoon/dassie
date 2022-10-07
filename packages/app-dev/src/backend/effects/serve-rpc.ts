import { applyWSSHandler } from "@trpc/server/adapters/ws"
import { WebSocketServer } from "ws"

import { readFileSync } from "node:fs"
import { createServer } from "node:https"
import { join } from "node:path"

import type { EffectContext } from "@dassie/lib-reactive"

import { LOCAL_FOLDER } from "../constants/paths"
import { DEBUG_RPC_PORT } from "../constants/ports"
import { AppRouter, appRouter } from "../rpc-routers/app-router"
import { validateCertificates } from "./validate-certificates"

const certificatePath = join(
  LOCAL_FOLDER,
  "tls/dev-rpc.localhost/web-dev-rpc.localhost.pem"
)
const keyPath = join(
  LOCAL_FOLDER,
  "tls/dev-rpc.localhost/web-dev-rpc.localhost-key.pem"
)

export const listenForRpcWebSocket = async (sig: EffectContext) => {
  await sig.run(validateCertificates, {
    id: "dev",
    certificates: [
      {
        type: "web",
        commonName: "dev-rpc.localhost",
        certificatePath,
        keyPath,
      },
    ],
  })

  const httpsServer = createServer({
    cert: readFileSync(certificatePath),
    key: readFileSync(keyPath),
  })

  const wss = new WebSocketServer({ server: httpsServer })
  const { broadcastReconnectNotification } = applyWSSHandler<AppRouter>({
    wss,
    router: appRouter,
    createContext: () => ({ reactor: sig.reactor }),
  })

  httpsServer.listen(DEBUG_RPC_PORT)

  sig.onCleanup(() => {
    broadcastReconnectNotification()
    httpsServer.close()
    wss.close()
  })
}
