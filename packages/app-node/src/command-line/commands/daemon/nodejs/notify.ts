import { execSync } from "node:child_process"

import { systemd as logger } from "../../../../backend/logger/instances"

export function notifySystemdReady() {
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

  execSync("systemd-notify --ready")
}
