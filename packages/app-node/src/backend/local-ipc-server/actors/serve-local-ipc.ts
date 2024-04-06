import { unlinkSync } from "node:fs"
import { createServer } from "node:net"

import { createActor } from "@dassie/lib-reactive"
import {
  createNodejsSocketAdapter,
  createServer as createRpcServer,
} from "@dassie/lib-rpc/server"
import { isErrorWithCode } from "@dassie/lib-type-utils"

import { DassieActorContext } from "../../base/types/dassie-base"
import { EnvironmentConfig } from "../../config/environment-config"
import { ipc as logger } from "../../logger/instances"
import { appRouter } from "../../rpc-server/app-router"
import {
  getSocketActivationFileDescriptors,
  getSocketActivationState,
} from "../../systemd/socket-activation"

const SOCKET_ACTIVATION_NAME_IPC = "dassie-ipc.socket"

const getListenTargets = (
  socketPath: string,
): readonly (string | { fd: number })[] => {
  const socketActivationState = getSocketActivationState()

  if (socketActivationState) {
    const fds = getSocketActivationFileDescriptors(
      socketActivationState,
      SOCKET_ACTIVATION_NAME_IPC,
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

export const ServeLocalIpcActor = () =>
  createActor((sig: DassieActorContext) => {
    const { ipcSocketPath } = sig.reactor.use(EnvironmentConfig)

    const server = createServer((socket) => {
      rpcServer.handleConnection({
        connection: createNodejsSocketAdapter(socket),
        context: {
          sig,
          isAuthenticated: true,
        },
      })
    })

    const rpcServer = createRpcServer({
      router: appRouter,
    })

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
