import { startNodeServer, startWalletServer } from "../src"
import { NODES } from "../src/constants/development-nodes"

await startNodeServer(NODES)
await startWalletServer()
