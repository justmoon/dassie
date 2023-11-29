import { execSync } from "node:child_process"

import { createActor } from "@dassie/lib-reactive"

import { DaemonActor } from "../daemon"
import { systemd as logger } from "../logger/instances"

export const NotifySystemdActor = () =>
  createActor((sig) => {
    const notifySocket = process.env["NOTIFY_SOCKET"]

    if (!notifySocket) return

    logger.assert(
      notifySocket.startsWith("@") || notifySocket.startsWith("/"),
      "NOTIFY_SOCKET must start with @ or /",
    )
    logger.assert(
      notifySocket.length >= 2,
      "NOTIFY_SOCKET must be at least 2 characters long",
    )

    // TODO: Use a native module to send the notification

    // The root actor promise will resolve when the application has finished
    // starting up. This is when we want to send the notification.
    void sig.reactor.use(DaemonActor).promise.then(() => {
      execSync("systemd-notify --ready")
    })
  })
