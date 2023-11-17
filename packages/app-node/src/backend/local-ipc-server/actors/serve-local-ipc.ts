import { unlinkSync } from "node:fs"
import { createServer } from "node:net"

import { createActor } from "@dassie/lib-reactive"
import { createSocketHandler } from "@dassie/lib-trpc-ipc/adapter"
import { isErrorWithCode } from "@dassie/lib-type-utils"

import { EnvironmentConfigSignal } from "../../config/environment-config"
import { ipc as logger } from "../../logger/instances"
import {
  getSocketActivationFileDescriptors,
  getSocketActivationState,
} from "../../systemd/socket-activation"
import { appRouter } from "../../trpc-server/app-router"

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
  createActor((sig) => {
    const { ipcSocketPath } = sig.readKeysAndTrack(EnvironmentConfigSignal, [
      "ipcSocketPath",
    ])

    const handler = createSocketHandler({
      router: appRouter,
      createContext: () => ({
        sig,
        user: true,
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
