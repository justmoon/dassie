import { InputConfig, fromEnvironment, fromPartialConfig } from "./config"
import HttpService from "./services/http"
import WebSocketService from "./services/websocket"

const start = async (inputConfig?: InputConfig) => {
  const config = inputConfig
    ? await fromPartialConfig(inputConfig)
    : await fromEnvironment()

  const http = new HttpService({ config })

  const ws = new WebSocketService({ config, http })
}

export default start
