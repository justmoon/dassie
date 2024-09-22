import { createServer } from "node:net"

import { createActor } from "@dassie/lib-reactive"
import { createNodejsSocketAdapter } from "@dassie/lib-rpc/server"

import type { SystemdReactor } from "."
import { ServeLocalIpcActor } from "../../../../local-ipc-server/actors/serve-local-ipc"
import { ipc as logger } from "../../../../logger/instances"
import { getSocketActivationFileDescriptors } from "./socket-activation"

const SOCKET_ACTIVATION_NAME_IPC = "dassie-ipc.socket"

function handleError(error: unknown) {
  logger.error("local ipc server error", { error })
}

export const ServeIpcSocketActor = (reactor: SystemdReactor) => {
  const serveLocalIpcActor = reactor.use(ServeLocalIpcActor)

  const server = createServer()

  const fileDescriptors = getSocketActivationFileDescriptors(
    reactor.base.socketActivationState,
    SOCKET_ACTIVATION_NAME_IPC,
  )
  logger.debug?.("using socket activation to create ipc socket", {
    fileDescriptors,
  })

  // socket activation can only be done once per process, which is why the call
  // to `server.listen` is outside of the createActor function
  for (const fd of fileDescriptors) {
    server.listen({ fd })
  }

  function handleConnection(socket: NodeJS.Socket) {
    serveLocalIpcActor.api.handleConnection.tell(
      createNodejsSocketAdapter(socket),
    )
  }

  return createActor((sig) => {
    server.addListener("connection", handleConnection)
    server.addListener("error", handleError)

    sig.onCleanup(() => {
      server.removeListener("connection", handleConnection)
      server.removeListener("error", handleError)
      server.close()
    })
  })
}
