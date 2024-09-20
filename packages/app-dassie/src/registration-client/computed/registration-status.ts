import {
  type InferSignalType,
  type Reactor,
  createComputed,
} from "@dassie/lib-reactive"

import { EnvironmentConfig } from "../../config/environment-config"
import { NodeIdSignal } from "../../ilp-connector/computed/node-id"
import { BootstrapNodeListsSignal } from "../../peer-protocol/signals/bootstrap-node-lists"
import { TARGET_REGISTRATION_THRESHOLD } from "../constants/threshold"

export const RegistrationStatusSignal = (reactor: Reactor) => {
  const { bootstrapNodes } = reactor.use(EnvironmentConfig)

  const idToHostnameMap = new Map<string, string>()
  for (const { id, url } of bootstrapNodes) {
    idToHostnameMap.set(id, new URL(url).hostname)
  }

  return createComputed(reactor, (sig) => {
    const bootstrapNodeLists = sig.readAndTrack(BootstrapNodeListsSignal)
    const ourNodeId = sig.readAndTrack(NodeIdSignal)

    const nodes = [...bootstrapNodeLists.entries()].map(
      ([nodeId, nodeList]) => ({
        nodeId,
        hostname: idToHostnameMap.get(nodeId),
        registered: nodeList.entries.includes(ourNodeId),
      }),
    )

    const registrationRatio =
      nodes.reduce((count, { registered }) => {
        return count + (registered ? 1 : 0)
      }, 0) / nodes.length

    return {
      nodes,
      registrationRatio,
      threshold: TARGET_REGISTRATION_THRESHOLD,
    }
  })
}

export type RegistrationStatus = InferSignalType<
  typeof RegistrationStatusSignal
>
