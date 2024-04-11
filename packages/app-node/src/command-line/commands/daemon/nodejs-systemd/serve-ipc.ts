import { createServer } from "node:net"

import { createActor } from "@dassie/lib-reactive"
import { createNodejsSocketAdapter } from "@dassie/lib-rpc/server"

import type { SystemdReactor } from "."
import { ServeLocalIpcActor } from "../../../../backend/local-ipc-server/actors/serve-local-ipc"
import { ipc as logger } from "../../../../backend/logger/instances"
import { getSocketActivationFileDescriptors } from "./socket-activation"

export const SOCKET_ACTIVATION_NAME_IPC = "dassie-ipc.socket"
export const ServeIpcSocketActor = (reactor: SystemdReactor) => {
  const serveLocalIpcActor = reactor.use(ServeLocalIpcActor)

  return createActor((sig) => {
    const server = createServer((socket) => {
      serveLocalIpcActor.api.handleConnection.tell(
        createNodejsSocketAdapter(socket),
      )
    })

    server.addListener("error", (error) => {
      logger.error("local ipc server error", { error })
    })

    const fileDescriptors = getSocketActivationFileDescriptors(
      reactor.base.socketActivationState,
      SOCKET_ACTIVATION_NAME_IPC,
    )
    logger.debug("using socket activation to create ipc socket", {
      fileDescriptors,
    })

    for (const fd of fileDescriptors) {
      server.listen({ fd })
    }

    sig.onCleanup(() => {
      server.close()
    })
  })
}
