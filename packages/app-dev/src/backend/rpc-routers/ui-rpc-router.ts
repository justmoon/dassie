import launchEditor from "launch-editor"
import { z } from "zod"

import {
  subscribeToSignal,
  subscribeToStore,
  subscribeToTopic,
} from "@dassie/lib-reactive-trpc/server"

import { LogsStore } from "../../common/stores/logs"
import { SecurityTokenSignal } from "../signals/security-token"
import { ActiveNodesStore } from "../stores/active-nodes"
import {
  EnvironmentSettingsStore,
  VALID_PEERING_MODES,
} from "../stores/environment-settings"
import { PeeringStateStore } from "../stores/peering-state"
import { PeerTrafficTopic } from "../topics/peer-traffic"
import { trpc } from "./trpc"

export const uiRpcRouter = trpc.router({
  getSecurityToken: trpc.procedure.query(({ ctx: { sig } }) =>
    sig.reactor.use(SecurityTokenSignal).read(),
  ),
  addRandomNode: trpc.procedure
    // TRPC seems to throw an error when using superjson as a transformer on a method with no parameters.
    .input(z.object({}))
    .mutation(({ ctx: { sig } }) => {
      sig.reactor.use(ActiveNodesStore).addNode()
    }),
  setPeeringMode: trpc.procedure
    .input(z.enum(VALID_PEERING_MODES))
    .mutation(({ ctx: { sig }, input }) => {
      sig.reactor.use(EnvironmentSettingsStore).setPeeringMode(input)
    }),
  subscribeToNodes: trpc.procedure.subscription(({ ctx: { sig } }) => {
    return subscribeToSignal(sig, ActiveNodesStore)
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
  subscribeToEnvironmentSettings: trpc.procedure.subscription(
    ({ ctx: { sig } }) => {
      return subscribeToSignal(sig, EnvironmentSettingsStore)
    },
  ),
})

export type UiRpcRouter = typeof uiRpcRouter
