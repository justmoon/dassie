import HttpService from "./services/http"
import WebSocketService from "./services/websocket"
import { fromEnvironment, fromPartialConfig, InputConfig } from "./config"

const start = async (inputConfig?: InputConfig) => {
  const config = inputConfig
    ? await fromPartialConfig(inputConfig)
    : await fromEnvironment()

  const http = new HttpService({ config })

  const ws = new WebSocketService({ config, http })
}

export default start
