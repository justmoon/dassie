import launchEditor from "launch-editor"
import { z } from "zod"

import { subscribeToSignal } from "@dassie/lib-reactive-trpc/server"

import { activeNodesStore } from "../stores/active-nodes"
import {
  VALID_PEERING_MODES,
  environmentSettingsStore,
} from "../stores/environment-settings"
import { trpc } from "./trpc"

export const uiRpcRouter = trpc.mergeRouters(
  trpc.router({
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
  })
)

export type UiRpcRouter = typeof uiRpcRouter
