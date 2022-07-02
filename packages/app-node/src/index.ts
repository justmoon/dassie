import { EventBroker } from "@xen-ilp/lib-events"
import { State } from "@xen-ilp/lib-state"

import XenSerializer from "./actors/xen-serializer"
import { fromEnvironment, fromPartialConfig } from "./config"
import type { InputConfig } from "./config"
import HttpService from "./services/http"
import PeerTable from "./services/peer-table"
import SigningService from "./services/signing"
import WebSocketService from "./services/websocket"

const start = async (inputConfig?: InputConfig) => {
  const config = inputConfig
    ? await fromPartialConfig(inputConfig)
    : await fromEnvironment()

  const signing = new SigningService({ config })

  const state = new State()

  const eventBroker = new EventBroker()

  const xenSerializer = new XenSerializer({ eventBroker })

  const peerTable = new PeerTable({
    config,
    signing,
    state,
    eventBroker,
  })

  const http = new HttpService({ config, eventBroker })

  const ws = new WebSocketService({ config, http })

  return {
    config,
    signing,
    state,
    eventBroker,
    xenSerializer,
    peerTable,
    http,
    ws,
  }
}

export type { InputConfig } from "./config"

export default start
