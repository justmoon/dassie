import { AxiosError } from "axios"
import {
  allowErrorProps,
  registerClass,
  SuperJSON as superjson,
} from "superjson"
import { WebSocketServer } from "ws"

import { readFileSync } from "node:fs"
import { createServer as createHttpsServer } from "node:https"
import { join } from "node:path"

import { Reactor, createActor } from "@dassie/lib-reactive"
import {
  createServer as createRpcServer,
  createWebSocketAdapter,
} from "@dassie/lib-rpc/server"

import { LOCAL_FOLDER } from "../constants/paths"
import { DEBUG_RPC_PORT } from "../constants/ports"
import { appRouter } from "../rpc-routers/app-router"
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

    allowErrorProps("stack", "cause")
    registerClass(AggregateError, {
      allowProps: ["errors", "message", "stack", "cause"],
    })
    registerClass(AxiosError, {
      allowProps: ["code", "errors", "message", "name", "config", "cause"],
    })

    const httpsServer = createHttpsServer({
      cert: readFileSync(certificatePath),
      key: readFileSync(keyPath),
    })

    const wss = new WebSocketServer({ server: httpsServer })

    const rpcServer = createRpcServer({
      router: appRouter,
      transformer: superjson,
    })

    wss.on("connection", (websocket) => {
      rpcServer.handleConnection({
        connection: createWebSocketAdapter(websocket),
        context: { sig, reactor },
      })
    })

    httpsServer.listen(DEBUG_RPC_PORT)

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
          if (error) reject(error)
          else resolve()
        })
      })
    })
  })
