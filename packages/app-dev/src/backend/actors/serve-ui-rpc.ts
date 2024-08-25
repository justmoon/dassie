import { WebSocketServer } from "ws"

import { readFileSync } from "node:fs"
import { createServer as createHttpsServer } from "node:https"
import path from "node:path"

import { Reactor, createActor } from "@dassie/lib-reactive"
import {
  createServer as createRpcServer,
  createWebSocketAdapter,
} from "@dassie/lib-rpc/server"

import { transformer } from "../../common/utils/transformer"
import { LOCAL_FOLDER } from "../constants/paths"
import { DEBUG_UI_RPC_PORT } from "../constants/ports"
import { uiRpcRouter } from "../rpc-routers/ui-rpc-router"
import { validateCertificates } from "../utils/validate-certificates"

const certificatePath = path.join(
  LOCAL_FOLDER,
  "tls/dev-rpc.localhost/web-dev-rpc.localhost.pem",
)
const keyPath = path.join(
  LOCAL_FOLDER,
  "tls/dev-rpc.localhost/web-dev-rpc.localhost-key.pem",
)

const CONNECTION_CLOSE_TIMEOUT = 250

export const ServeUiRpcActor = (reactor: Reactor) =>
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

    const httpsServer = createHttpsServer({
      cert: readFileSync(certificatePath),
      key: readFileSync(keyPath),
    })

    const wss = new WebSocketServer({ server: httpsServer })

    const rpcServer = createRpcServer({
      router: uiRpcRouter,
      transformer,
    })

    wss.on("connection", (websocket) => {
      rpcServer.handleConnection({
        connection: createWebSocketAdapter(websocket),
        context: { sig, reactor },
      })
    })

    httpsServer.listen(DEBUG_UI_RPC_PORT)

    sig.onCleanup(async () => {
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
          // eslint-disable-next-line @typescript-eslint/prefer-promise-reject-errors
          if (error) reject(error)
          else resolve()
        })
      })
    })
  })
