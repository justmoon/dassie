import { initTRPC } from "@trpc/server"
import { z } from "zod"

import { unlinkSync } from "node:fs"
import { createServer } from "node:net"

import { createLogger } from "@dassie/lib-logger"
import { ActorContext, createActor } from "@dassie/lib-reactive"
import { createSocketHandler } from "@dassie/lib-trpc-ipc/adapter"
import { isErrorWithCode } from "@dassie/lib-type-utils"

import { environmentConfigSignal } from "../../config/environment-config"
import { databasePlain } from "../../database/open-database"
import {
  getSocketActivationFileDescriptors,
  getSocketActivationState,
} from "../../systemd/socket-activation"

const logger = createLogger("das:local-ipc-server")

const SOCKET_ACTIVATION_NAME_IPC = "dassie-ipc.socket"

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

const getListenTargets = (
  socketPath: string
): readonly (string | { fd: number })[] => {
  const socketActivationState = getSocketActivationState()

  if (socketActivationState) {
    const fds = getSocketActivationFileDescriptors(
      socketActivationState,
      SOCKET_ACTIVATION_NAME_IPC
    )
    logger.debug("using socket activation to create ipc socket", { fds })
    return fds.map((fd) => ({ fd }))
  }

  try {
    unlinkSync(socketPath)
  } catch (error) {
    if (!isErrorWithCode(error, "ENOENT")) {
      throw error
    }
  }

  logger.debug("creating local ipc socket", { path: socketPath })

  return [socketPath]
}

export const serveLocalIpc = () =>
  createActor((sig) => {
    const { ipcSocketPath } = sig.getKeys(environmentConfigSignal, [
      "ipcSocketPath",
    ])

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

    for (const listenTarget of getListenTargets(ipcSocketPath)) {
      server.listen(listenTarget)
    }

    sig.onCleanup(() => {
      server.close()
    })
  })
