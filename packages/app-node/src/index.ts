import { fromEnvironment, fromPartialConfig } from "./config"
import type { InputConfig } from "./config"
import HttpService from "./services/http"
import PeerTable from "./services/peer-table"
import SigningService from "./services/signing"
import State from "./services/state"
import WebSocketService from "./services/websocket"

const start = async (inputConfig?: InputConfig) => {
  const config = inputConfig
    ? await fromPartialConfig(inputConfig)
    : await fromEnvironment()

  const signing = new SigningService({ config })

  const state = new State()

  const peerTable = new PeerTable({ config, signing, state })

  const http = new HttpService({ config, peerTable: peerTable })

  const ws = new WebSocketService({ config, http })

  return { config, http, ws, peerManager: peerTable }
}

export type { InputConfig } from "./config"

export default start
