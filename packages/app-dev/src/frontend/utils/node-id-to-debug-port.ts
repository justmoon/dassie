import { NODES_DEBUG_START_PORT } from "../../backend/constants/ports"

export const nodeIdToDebugUrl = (nodeId: string) => {
  return `ws://localhost:${
    Number(nodeId.slice(1)) + NODES_DEBUG_START_PORT - 1
  }`
}
