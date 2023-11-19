import { z } from "zod"

import {
  FactoryNameSymbol,
  Reactor,
  isActor,
  isComputed,
  isSignal,
  isTopic,
} from "@dassie/lib-reactive"
import {
  subscribeToSignal,
  subscribeToTopic,
} from "@dassie/lib-reactive-trpc/server"
import { isObject } from "@dassie/lib-type-utils"

import { LedgerStore } from "../../accounting/stores/ledger"
import { EnvironmentConfigSignal } from "../../config/environment-config"
import { NodeTableStore } from "../../peer-protocol/stores/node-table"
import { RoutingTableSignal } from "../../routing/signals/routing-table"
import { prettyFormat } from "../../utils/pretty-format"
import { protectedProcedure } from "../middlewares/auth"
import { trpc } from "../trpc-context"

export type ContextKeyTuple =
  | [id: number, name: string, type: "actor", parent: number]
  | [id: number, name: string, type: "signal"]
  | [id: number, name: string, type: "computed"]
  | [id: number, name: string, type: "topic"]
  | [id: number, name: string, type: "other"]

const getContextKeyTuple = (
  id: number,
  item: unknown,
  reactor: Reactor,
  reverseMap: Map<unknown, number>,
): ContextKeyTuple => {
  const name =
    item && isObject(item) && typeof item[FactoryNameSymbol] === "string"
      ? item[FactoryNameSymbol]
      : "anonymous"

  if (isActor(item)) {
    return [
      id,
      name,
      "actor",
      reverseMap.get(reactor.debug?.getActorParent(item)) ?? -1,
    ]
  }

  if (isComputed(item)) {
    return [id, name, "computed"]
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

    if (!context) {
      return []
    }

    const reverseMap = new Map<unknown, number>()

    for (const entry of context.values()) {
      const item = entry.reference.deref()
      if (typeof item === "object" && item != null) {
        reverseMap.set(item, entry.uniqueId)
      }
    }

    return context
      ? [...context.values()].map<ContextKeyTuple>((entry) => {
          const item = entry.reference.deref()
          return getContextKeyTuple(
            entry.uniqueId,
            item,
            sig.reactor,
            reverseMap,
          )
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

      return { value: prettyFormat(item.read()) }
    }),
  subscribeToTopic: protectedProcedure
    .input(z.number())
    .subscription(({ ctx: { sig }, input: id }) => {
      const item = sig.reactor.debug?.getContext()?.get(id)?.reference.deref()
      if (!isTopic(item)) {
        throw new TypeError("Item is not a topic")
      }

      return subscribeToTopic(sig, item, {
        transform: (item) => prettyFormat(item),
      })
    }),
})
