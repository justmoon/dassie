import XenSerializer from "./actors/xen-serializer"
import { fromEnvironment, fromPartialConfig } from "./config"
import type { InputConfig } from "./config"
import HttpService from "./services/http"
import MessageBroker from "./services/message-broker"
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

  const messageBroker = new MessageBroker()

  const xenSerializer = new XenSerializer({ messageBroker })

  const peerTable = new PeerTable({ config, signing, state, messageBroker })

  const http = new HttpService({ config, messageBroker })

  const ws = new WebSocketService({ config, http })

  return {
    config,
    signing,
    state,
    messageBroker,
    xenSerializer,
    peerTable,
    http,
    ws,
  }
}

export type { InputConfig } from "./config"

export default start
