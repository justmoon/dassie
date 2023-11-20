import launchEditor from "launch-editor"
import { z } from "zod"

import {
  subscribeToSignal,
  subscribeToStore,
  subscribeToTopic,
} from "@dassie/lib-reactive-trpc/server"

import { LogsStore } from "../../common/stores/logs"
import { ActiveNodeIdsComputed } from "../computed/active-node-ids"
import { SecurityTokenSignal } from "../signals/security-token"
import { PeeringStateStore } from "../stores/peering-state"
import { ScenarioStore } from "../stores/scenario"
import { PeerTrafficTopic } from "../topics/peer-traffic"
import { trpc } from "./trpc"

export const uiRpcRouter = trpc.router({
  getSecurityToken: trpc.procedure.query(({ ctx: { sig } }) =>
    sig.read(SecurityTokenSignal),
  ),
  addRandomNode: trpc.procedure
    // TRPC seems to throw an error when using superjson as a transformer on a method with no parameters.
    .input(z.object({}))
    .mutation(({ ctx: { sig } }) => {
      sig.reactor.use(ScenarioStore).addNode()
    }),
  subscribeToNodes: trpc.procedure.subscription(({ ctx: { sig } }) => {
    return subscribeToSignal(sig, ActiveNodeIdsComputed)
  }),
  openFile: trpc.procedure.input(z.string()).mutation(({ input }) => {
    launchEditor(input)
  }),
  subscribeToLogs: trpc.procedure.subscription(({ ctx: { sig } }) => {
    return subscribeToStore(sig, LogsStore)
  }),
  subscribeToPeerTraffic: trpc.procedure.subscription(({ ctx: { sig } }) => {
    return subscribeToTopic(sig, PeerTrafficTopic)
  }),
  subscribeToPeeringState: trpc.procedure.subscription(({ ctx: { sig } }) => {
    return subscribeToSignal(sig, PeeringStateStore)
  }),
  subscribeToScenario: trpc.procedure.subscription(({ ctx: { sig } }) => {
    return subscribeToSignal(sig, ScenarioStore)
  }),
})

export type UiRpcRouter = typeof uiRpcRouter
