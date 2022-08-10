export const nodeIdToDebugUrl = (nodeId: string) => {
  return `ws://localhost:${Number(nodeId.slice(1)) + 5000 - 1}`
}
