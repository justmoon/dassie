import { createStore } from "@dassie/lib-reactive"

export const peeringStateStore = () =>
  createStore({} as Record<string, string[]>, {
    updateNodePeers: (nodeId: string, peers: string[]) => (state) => ({
      ...state,
      [nodeId]: peers,
    }),
  })
