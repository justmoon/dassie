import { Reactor, createMapped, createSignal } from "@dassie/lib-reactive"

import { generateNodeConfig } from "../utils/generate-node-config"
import { activeNodesStore } from "./active-nodes"
import { environmentSettingsStore } from "./environment-settings"

export const nodeConfigMap = (reactor: Reactor) =>
  createMapped(activeNodesStore, (nodeId) => {
    const environmentSettings = reactor.use(environmentSettingsStore).read()

    return createSignal(generateNodeConfig(nodeId, environmentSettings))
  })
