import { type Reactor, createActor } from "@dassie/lib-reactive"
import { createRuntime } from "@dassie/lib-reactive-io/node"
import { tell } from "@dassie/lib-type-utils"

import { DaemonActor } from "../../../../daemon"
import { systemd as logger } from "../../../../logger/instances"
import { LogToConsoleActor } from "../nodejs/log-to-console"
import { notifySystemdReady } from "./notify"
import { ServeHttpActor } from "./serve-http"
import { ServeHttpsActor } from "./serve-https"
import { ServeIpcSocketActor } from "./serve-ipc"
import {
  type SocketActivationState,
  getSocketActivationState,
} from "./socket-activation"

interface SystemdBase {
  socketActivationState: SocketActivationState
}
export type SystemdReactor = Reactor<SystemdBase>

export const NodejsSystemdDaemonActor = (reactor: Reactor) =>
  createActor(async (sig) => {
    sig.run(LogToConsoleActor)

    const socketActivationState = getSocketActivationState()

    if (!socketActivationState) {
      logger.error(
        "could not find systemd socket activation environment; please make sure the daemon is running as a systemd service and check your systemd configuration; exiting",
      )
      process.exitCode = 1
      tell(reactor.dispose)
      return
    }

    const systemdReactor = sig.withBase({
      socketActivationState,
    })
    systemdReactor.run(ServeHttpActor)
    systemdReactor.run(ServeIpcSocketActor)
    systemdReactor.run(ServeHttpsActor)

    await sig.withBase(createRuntime()).run(DaemonActor)

    notifySystemdReady()
  })
