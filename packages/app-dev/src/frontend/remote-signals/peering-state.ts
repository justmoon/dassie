import { createRemoteSignal } from "../utils/remote-reactive"

export const peeringStateStore = () =>
  createRemoteSignal("peeringState", {} as Record<string, string[]>)
