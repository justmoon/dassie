import { startNodeServer, startWalletServer } from "../src"
import { NODES } from "../src/constants/development-nodes"

startNodeServer(NODES)
startWalletServer()
