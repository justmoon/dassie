import { unlinkSync } from "node:fs"
import { createServer } from "node:net"

import { type Reactor, createActor } from "@dassie/lib-reactive"
import { createNodejsSocketAdapter } from "@dassie/lib-rpc/server"
import { isErrorWithCode } from "@dassie/lib-type-utils"

import { EnvironmentConfig } from "../../../../backend/config/environment-config"
import { ServeLocalIpcActor } from "../../../../backend/local-ipc-server/actors/serve-local-ipc"
import { ipc as logger } from "../../../../backend/logger/instances"

export const SOCKET_ACTIVATION_NAME_IPC = "dassie-ipc.socket"
export const ServeIpcSocketActor = (reactor: Reactor) => {
  const serveLocalIpcActor = reactor.use(ServeLocalIpcActor)
  const { ipcSocketPath } = reactor.use(EnvironmentConfig)

  return createActor((sig) => {
    const server = createServer((socket) => {
      serveLocalIpcActor.api.handleConnection.tell(
        createNodejsSocketAdapter(socket),
      )
    })

    server.addListener("error", (error) => {
      logger.error("local ipc server error", { error })
    })

    logger.debug("listening on ipc socket", {
      path: ipcSocketPath,
    })

    try {
      unlinkSync(ipcSocketPath)
    } catch (error) {
      if (!isErrorWithCode(error, "ENOENT")) throw error
    }

    server.listen(ipcSocketPath)

    sig.onCleanup(() => {
      server.close()
    })
  })
}
