import launchEditor from "launch-editor"
import { z } from "zod"

import {
  subscribeToSignal,
  subscribeToStore,
  subscribeToTopic,
} from "@dassie/lib-reactive-trpc/server"

import { logsStore } from "../../common/stores/logs"
import { activeNodesStore } from "../stores/active-nodes"
import {
  VALID_PEERING_MODES,
  environmentSettingsStore,
} from "../stores/environment-settings"
import { peeringStateStore } from "../stores/peering-state"
import { peerTrafficTopic } from "../topics/peer-traffic"
import { trpc } from "./trpc"

export const uiRpcRouter = trpc.router({
  addRandomNode: trpc.procedure
    // TRPC seems to throw an error when using superjson as a transformer on a method with no parameters.
    .input(z.object({}))
    .mutation(({ ctx: { sig } }) => {
      sig.use(activeNodesStore).addNode()
    }),
  setPeeringMode: trpc.procedure
    .input(z.enum(VALID_PEERING_MODES))
    .mutation(({ ctx: { sig }, input }) => {
      sig.use(environmentSettingsStore).setPeeringMode(input)
    }),
  subscribeToNodes: trpc.procedure.subscription(({ ctx: { sig } }) => {
    return subscribeToSignal(sig, activeNodesStore)
  }),
  openFile: trpc.procedure.input(z.string()).mutation(({ input }) => {
    launchEditor(input)
  }),
  subscribeToLogs: trpc.procedure.subscription(({ ctx: { sig } }) => {
    return subscribeToStore(sig, logsStore)
  }),
  subscribeToPeerTraffic: trpc.procedure.subscription(({ ctx: { sig } }) => {
    return subscribeToTopic(sig, peerTrafficTopic)
  }),
  subscribeToPeeringState: trpc.procedure.subscription(({ ctx: { sig } }) => {
    return subscribeToSignal(sig, peeringStateStore)
  }),
  subscribeToEnvironmentSettings: trpc.procedure.subscription(
    ({ ctx: { sig } }) => {
      return subscribeToSignal(sig, environmentSettingsStore)
    }
  ),
})

export type UiRpcRouter = typeof uiRpcRouter
