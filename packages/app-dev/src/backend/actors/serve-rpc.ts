import { applyWSSHandler } from "@trpc/server/adapters/ws"
import { WebSocketServer } from "ws"

import { readFileSync } from "node:fs"
import { createServer } from "node:https"
import { join } from "node:path"

import { Reactor, createActor } from "@dassie/lib-reactive"

import { LOCAL_FOLDER } from "../constants/paths"
import { DEBUG_RPC_PORT } from "../constants/ports"
import { type AppRouter, appRouter } from "../rpc-routers/app-router"
import { validateCertificates } from "../utils/validate-certificates"

const certificatePath = join(
  LOCAL_FOLDER,
  "tls/dev-rpc.localhost/web-dev-rpc.localhost.pem",
)
const keyPath = join(
  LOCAL_FOLDER,
  "tls/dev-rpc.localhost/web-dev-rpc.localhost-key.pem",
)

const CONNECTION_CLOSE_TIMEOUT = 250

export const ListenForRpcWebSocketActor = (reactor: Reactor) =>
  createActor(async (sig) => {
    await validateCertificates({
      id: "dev",
      certificates: [
        {
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
      createContext: () => ({ sig, reactor }),
    })

    httpsServer.listen(DEBUG_RPC_PORT)

    sig.onCleanup(async () => {
      broadcastReconnectNotification()

      await Promise.all(
        [...wss.clients].map((client) => {
          return new Promise<void>((resolve, reject) => {
            const timer = setTimeout(() => {
              client.terminate()
            }, CONNECTION_CLOSE_TIMEOUT)

            client.on("close", () => {
              clearTimeout(timer)
              resolve()
            })

            client.on("error", (error) => {
              clearTimeout(timer)
              reject(error)
            })

            client.close()
          })
        }),
      )

      wss.close()

      await new Promise<void>((resolve, reject) => {
        httpsServer.close((error: unknown) => {
          if (error) reject(error)
          else resolve()
        })
      })
    })
  })
