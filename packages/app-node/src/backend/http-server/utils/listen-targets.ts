import { http as logger } from "../../logger/instances"
import {
  getSocketActivationFileDescriptors,
  getSocketActivationState,
} from "../../systemd/socket-activation"

export const SOCKET_ACTIVATION_NAME_HTTP = "dassie-http.socket"
export const SOCKET_ACTIVATION_NAME_HTTPS = "dassie-https.socket"

export const getListenTargets = (
  port: number,
  secure: boolean,
): readonly (number | { fd: number })[] => {
  const socketActivationState = getSocketActivationState()

  if (socketActivationState) {
    const fds = getSocketActivationFileDescriptors(
      socketActivationState,
      secure ? SOCKET_ACTIVATION_NAME_HTTPS : SOCKET_ACTIVATION_NAME_HTTP,
    )
    logger.debug("using socket activation for web server", { fds })
    return fds.map((fd) => ({ fd }))
  }

  return [port]
}
