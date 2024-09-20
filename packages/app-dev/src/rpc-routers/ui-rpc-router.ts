import launchEditor from "launch-editor"
import { z } from "zod"

import {
  subscribeToSignal,
  subscribeToStore,
  subscribeToTopic,
} from "@dassie/lib-reactive-rpc/server"
import { createRouter } from "@dassie/lib-rpc/server"

import { ActiveNodeIdsComputed } from "../computed/active-node-ids"
import { scenarios } from "../scenarios"
import { ScenarioSignal } from "../signals/scenario"
import { SecurityTokenSignal } from "../signals/security-token"
import { AdditionalNodesStore } from "../stores/additional-nodes"
import {
  DEFAULT_ADDITIONAL_NODE_START_INDEX,
  EnvironmentStore,
} from "../stores/environment"
import { LogsStore } from "../stores/logs"
import { PeeringStateStore } from "../stores/peering-state"
import { PeerTrafficTopic } from "../topics/peer-traffic"
import { baseRoute } from "./route-types/base"

export const uiRpcRouter = createRouter({
  getSecurityToken: baseRoute.query(({ context: { sig } }) =>
    sig.read(SecurityTokenSignal),
  ),
  addRandomNode: baseRoute
    // TRPC seems to throw an error when using superjson as a transformer on a method with no parameters.
    .input(z.object({}))
    .mutation(({ context: { sig } }) => {
      const environmentStore = sig.reactor.use(EnvironmentStore)
      const additionalNodesStore = sig.reactor.use(AdditionalNodesStore)
      const startIndex =
        environmentStore.read().additionalNodeStartIndex ??
        DEFAULT_ADDITIONAL_NODE_START_INDEX
      additionalNodesStore.act.addNode(
        startIndex + additionalNodesStore.read().size,
      )
    }),
  getBuiltinScenarios: baseRoute.query(() => {
    return Object.entries(scenarios).map(([id, { name, description }]) => ({
      id: id as keyof typeof scenarios,
      name,
      description,
    }))
  }),
  subscribeToNodes: baseRoute.subscription(({ context: { sig } }) => {
    return subscribeToSignal(sig, ActiveNodeIdsComputed)
  }),
  openFile: baseRoute.input(z.string()).mutation(({ input }) => {
    launchEditor(input)
  }),
  subscribeToLogs: baseRoute.subscription(({ context: { sig } }) => {
    return subscribeToStore(sig, LogsStore)
  }),
  subscribeToPeerTraffic: baseRoute.subscription(({ context: { sig } }) => {
    return subscribeToTopic(sig, PeerTrafficTopic)
  }),
  subscribeToPeeringState: baseRoute.subscription(({ context: { sig } }) => {
    return subscribeToSignal(sig, PeeringStateStore)
  }),
  subscribeToScenario: baseRoute.subscription(({ context: { sig } }) => {
    return subscribeToSignal(sig, ScenarioSignal)
  }),
  setScenario: baseRoute
    .input(
      z.string().transform((scenario, context) => {
        if (!Object.hasOwn(scenarios, scenario)) {
          context.addIssue({
            code: "invalid_enum_value",
            received: scenario,
            options: Object.keys(scenarios),
          })
          return z.NEVER
        }

        return scenario as keyof typeof scenarios
      }),
    )
    .mutation(({ context: { sig }, input: scenario }) => {
      sig.reactor.use(ScenarioSignal).write(scenario)
      sig.reactor.use(AdditionalNodesStore).act.clear()
    }),
})

export type UiRpcRouter = typeof uiRpcRouter
