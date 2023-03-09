import { createRemoteSignal } from "../utils/remote-reactive"

export const activeNodesStore = () =>
  createRemoteSignal("activeNodes", undefined)
