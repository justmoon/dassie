import { State } from "@xen-ilp/lib-state"

import { startDebugUiServer, startWalletServer } from "../src"
import { NODES } from "../src/constants/development-nodes"
import LogManager from "./classes/log-manager"
import NodeServer from "./servers/node-server"
import UiRpcServer from "./servers/ui-rpc-server"
import eventBroker from "./services/event-broker"
import { register as registerLoggerService } from "./services/logger"

const start = async () => {
  registerLoggerService()

  const state = new State()

  const logManager = new LogManager({ state, eventBroker })
  const uiRpcServer = new UiRpcServer({ eventBroker, logManager })

  const nodeServer = new NodeServer({ eventBroker })

  await nodeServer.start(NODES)
  await startWalletServer()
  await startDebugUiServer()

  return {
    state,
    eventBroker,
    logManager,
    uiRpcServer,
  }
}

export default start
