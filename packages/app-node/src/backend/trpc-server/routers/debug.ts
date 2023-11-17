import { z } from "zod"

import {
  FactoryNameSymbol,
  isActor,
  isSignal,
  isTopic,
} from "@dassie/lib-reactive"
import { subscribeToSignal } from "@dassie/lib-reactive-trpc/server"
import { isObject } from "@dassie/lib-type-utils"

import { LedgerStore } from "../../accounting/stores/ledger"
import { EnvironmentConfigSignal } from "../../config/environment-config"
import { NodeTableStore } from "../../peer-protocol/stores/node-table"
import { RoutingTableSignal } from "../../routing/signals/routing-table"
import { protectedProcedure } from "../middlewares/auth"
import { trpc } from "../trpc-context"

export type ContextKeyTuple =
  | [id: number, name: string, type: "actor"]
  | [id: number, name: string, type: "signal"]
  | [id: number, name: string, type: "topic"]
  | [id: number, name: string, type: "other"]

const getContextKeyTuple = (id: number, item: unknown): ContextKeyTuple => {
  const name =
    item && isObject(item) && typeof item[FactoryNameSymbol] === "string"
      ? item[FactoryNameSymbol]
      : "anonymous"

  if (isActor(item)) {
    return [id, name, "actor"]
  }

  if (isSignal(item)) {
    return [id, name, "signal"]
  }

  if (isTopic(item)) {
    return [id, name, "topic"]
  }

  return [id, name, "other"]
}

export const debugRouter = trpc.router({
  getLedger: protectedProcedure.query(({ ctx: { sig } }) => {
    return [...sig.reactor.use(LedgerStore).getAccounts("")]
  }),
  subscribeConfig: protectedProcedure.subscription(({ ctx: { sig } }) => {
    return subscribeToSignal(sig, EnvironmentConfigSignal)
  }),
  subscribeNodeTable: protectedProcedure.subscription(({ ctx: { sig } }) => {
    return subscribeToSignal(sig, NodeTableStore)
  }),
  subscribeRoutingTable: protectedProcedure.subscription(({ ctx: { sig } }) => {
    return subscribeToSignal(sig, RoutingTableSignal)
  }),
  getContextKeys: protectedProcedure.query(({ ctx: { sig } }) => {
    const context = sig.reactor.debug?.getContext()
    return context
      ? [...context.values()].map<ContextKeyTuple>((entry) => {
          const item = entry.reference.deref()
          return getContextKeyTuple(entry.uniqueId, item)
        })
      : []
  }),
  getSignalState: protectedProcedure
    .input(z.number())
    .query(({ ctx: { sig }, input: id }) => {
      const item = sig.reactor.debug?.getContext()?.get(id)?.reference.deref()
      if (!isSignal(item)) {
        throw new TypeError("Item is not a signal")
      }

      return { value: item.read() }
    }),
})
