import { fromEnvironment, fromPartialConfig } from "./config"
import type { InputConfig } from "./config"
import HttpService from "./services/http"
import PeerManager from "./services/peer-manager"
import SigningService from "./services/signing"
import WebSocketService from "./services/websocket"

const start = async (inputConfig?: InputConfig) => {
  const config = inputConfig
    ? await fromPartialConfig(inputConfig)
    : await fromEnvironment()

  const signing = new SigningService({ config })

  const peerManager = new PeerManager({ config, signing })

  const http = new HttpService({ config, peerManager })

  const ws = new WebSocketService({ config, http })

  return { config, http, ws, peerManager }
}

export type { InputConfig } from "./config"

export default start
