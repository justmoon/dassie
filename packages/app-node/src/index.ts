import { InputConfig, fromEnvironment, fromPartialConfig } from "./config"
import HttpService from "./services/http"
import PeerManager from "./services/peer-manager"
import WebSocketService from "./services/websocket"

const start = async (inputConfig?: InputConfig) => {
  const config = inputConfig
    ? await fromPartialConfig(inputConfig)
    : await fromEnvironment()

  const peerManager = new PeerManager({ config })

  const http = new HttpService({ config, peerManager })

  const ws = new WebSocketService({ config, http })

  return { config, http, ws, peerManager }
}

export default start
