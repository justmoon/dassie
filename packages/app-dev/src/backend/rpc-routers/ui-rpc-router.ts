import launchEditor from "launch-editor"
import { z } from "zod"

import {
  subscribeToSignal,
  subscribeToStore,
  subscribeToTopic,
} from "@dassie/lib-reactive-rpc/server"
import { createRouter } from "@dassie/lib-rpc/server"

import { LogsStore } from "../../common/stores/logs"
import { RunNodesActor } from "../actors/run-nodes"
import { ActiveNodeIdsComputed } from "../computed/active-node-ids"
import { SecurityTokenSignal } from "../signals/security-token"
import { PeeringStateStore } from "../stores/peering-state"
import { Scenario, ScenarioStore } from "../stores/scenario"
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
      sig.reactor.use(ScenarioStore).addNode()
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
    return subscribeToSignal(sig, ScenarioStore)
  }),
  setScenario: baseRoute
    .input(z.unknown().transform((value) => value as Scenario))
    .mutation(({ context: { sig }, input: scenario }) => {
      sig.reactor.use(PeeringStateStore).clear()
      sig.reactor.use(ScenarioStore).setScenario(scenario)
      sig.reactor.use(RunNodesActor).forceRestart()
    }),
})

export type UiRpcRouter = typeof uiRpcRouter
