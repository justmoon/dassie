import { execSync } from "node:child_process"

import { assert } from "@dassie/lib-logger"

import { systemd as logger } from "../../../../logger/instances"

export function notifySystemdReady() {
  const notifySocket = process.env["NOTIFY_SOCKET"]

  if (!notifySocket) return

  assert(
    logger,
    notifySocket.startsWith("@") || notifySocket.startsWith("/"),
    "NOTIFY_SOCKET must start with @ or /",
  )
  assert(
    logger,
    notifySocket.length >= 2,
    "NOTIFY_SOCKET must be at least 2 characters long",
  )

  // TODO: Use a native module to send the notification

  execSync("systemd-notify --ready")
}
