/* eslint-disable unicorn/no-process-exit */
import path from "node:path"

import { nodeIndexToDataPath } from "../src/backend/utils/generate-node-config"

const NODE_ID_REGEX = /^d[1-9]\d*$/
const nodeId = process.argv[2]

if (!nodeId || !NODE_ID_REGEX.test(nodeId)) {
  console.error("Usage: pnpm cli [nodeId] [...cli arguments]")
  console.error("")
  console.error(
    "This is a wrapper around the Dassie CLI that allows you to make calls to each",
  )
  console.error("of the Dassie instances in the development environment.")
  console.error("")
  console.error("Example: pnpm cli d1 init")
  process.exit(1)
}

process.argv.splice(2, 1)

const nodeIndex = Number.parseInt(nodeId.slice(1), 10) - 1

const nodePath = nodeIndexToDataPath(nodeIndex)
const socketPath = path.resolve(nodePath, "dassie.sock")

process.env["DASSIE_IPC_SOCKET_PATH"] = socketPath
;(global as unknown as { __DASSIE_VERSION__: string }).__DASSIE_VERSION__ =
  "dev"

await import("@dassie/app-node/src/command-line/entry")

export {}
