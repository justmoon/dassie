import { initTRPC } from "@trpc/server"
import { z } from "zod"

import { createServer } from "node:net"
import { resolve } from "node:path"

import { createLogger } from "@dassie/lib-logger"
import { ActorContext, createActor } from "@dassie/lib-reactive"
import { createSocketHandler } from "@dassie/lib-trpc-ipc/adapter"

import { environmentConfigSignal } from "../../config/environment-config"
import { databasePlain } from "../../database/open-database"

const logger = createLogger("das:local-ipc-server")

export interface IpcContext {
  sig: ActorContext
}

export const trpc = initTRPC.context<IpcContext>().create()

export const localIpcRouter = trpc.router({
  setNodeTlsConfiguration: trpc.procedure
    .input(
      z.object({
        accountUrl: z.string(),
        accountKey: z.string(),
      })
    )
    .mutation(({ input: { accountUrl, accountKey }, ctx: { sig } }) => {
      const database = sig.use(databasePlain)
      database.scalars.set("acme.account_url", accountUrl)
      database.scalars.set("acme.account_key", accountKey)
      return true
    }),
})

export type LocalIpcRouter = typeof localIpcRouter

export const serveLocalIpc = () =>
  createActor((sig) => {
    const { runtimePath } = sig.getKeys(environmentConfigSignal, [
      "runtimePath",
    ])
    const path =
      process.env["DASSIE_IPC_SOCKET_PATH"] ?? resolve(runtimePath, "ipc.sock")

    if (!path) return

    logger.debug("creating local ipc socket", { path })

    const handler = createSocketHandler({
      router: localIpcRouter,
      createContext: () => ({
        sig,
      }),
    })

    const server = createServer(handler)

    server.addListener("error", (error) => {
      logger.error("local ipc server error", { error })
    })

    server.listen(path)

    sig.onCleanup(() => {
      server.close()
    })
  })
