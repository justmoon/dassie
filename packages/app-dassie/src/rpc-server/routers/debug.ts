import { z } from "zod"

import {
  FactoryNameSymbol,
  type Reactor,
  isActor,
  isComputed,
  isSignal,
  isTopic,
} from "@dassie/lib-reactive"
import {
  subscribeToSignal,
  subscribeToStore,
  subscribeToTopic,
} from "@dassie/lib-reactive-rpc/server"
import { createRouter, createSubscription } from "@dassie/lib-rpc/server"
import { isObject } from "@dassie/lib-type-utils"

import {
  type LedgerAccount,
  LedgerStore,
  type Transfer,
} from "../../accounting/stores/ledger"
import { PendingTransfersTopic } from "../../accounting/topics/pending-transfers"
import { PostedTransfersTopic } from "../../accounting/topics/posted-transfers"
import { VoidedTransfersTopic } from "../../accounting/topics/voided-transfers"
import type { AccountPath } from "../../accounting/types/account-paths"
import { Database } from "../../database/open-database"
import { DATABASE_TABLE_IDS } from "../../database/schema"
import { LogsStore } from "../../logger/stores/logs"
import { RoutingTableSignal } from "../../routing/signals/routing-table"
import { prettyFormat } from "../../utils/pretty-format"
import { protectedRoute } from "../route-types/protected"

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
    item && isObject(item) && typeof item[FactoryNameSymbol] === "string" ?
      item[FactoryNameSymbol]
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

const [DATABASE_FIRST_TABLE_ID, ...DATABASE_OTHER_TABLE_IDS] =
  DATABASE_TABLE_IDS

export const debugRouter = createRouter({
  getLedger: protectedRoute.query(({ context: { sig } }) => {
    return [...sig.reactor.use(LedgerStore).getAccounts("")]
  }),
  subscribeToLogs: protectedRoute.subscription(({ context: { sig } }) => {
    return subscribeToStore(sig, LogsStore)
  }),
  subscribeRoutingTable: protectedRoute.subscription(({ context: { sig } }) => {
    return subscribeToSignal(sig, RoutingTableSignal)
  }),
  getContextKeys: protectedRoute.query(({ context: { sig } }) => {
    const context = sig.reactor.debug?.getContext()

    if (!context) {
      return []
    }

    const reverseMap = new Map<unknown, number>()

    for (const entry of context.values()) {
      const item = entry.reference.deref()
      if (item != null && typeof item === "object") {
        reverseMap.set(item, entry.uniqueId)
      }
    }

    return [...context.values()].map<ContextKeyTuple>((entry) => {
      const item = entry.reference.deref()
      return getContextKeyTuple(entry.uniqueId, item, sig.reactor, reverseMap)
    })
  }),
  getSignalState: protectedRoute
    .input(z.number())
    .query(({ context: { sig }, input: id }) => {
      const item = sig.reactor.debug?.getContext().get(id)?.reference.deref()
      if (!isSignal(item)) {
        throw new TypeError("Item is not a signal")
      }

      return { value: prettyFormat(item.read()) }
    }),
  subscribeToTopic: protectedRoute
    .input(z.number())
    .subscription(({ context: { sig }, input: id }) => {
      const item = sig.reactor.debug?.getContext().get(id)?.reference.deref()
      if (!isTopic(item)) {
        throw new TypeError("Item is not a topic")
      }

      return createSubscription<string>((onData) => {
        return subscribeToTopic(
          sig,
          item,
        )((data) => {
          onData(prettyFormat(data))
        })
      })
    }),
  getDatabaseTables: protectedRoute.query(({ context: { sig } }) => {
    const database = sig.reactor.use(Database)

    return Object.entries(database.schema.tables).map(([id, table]) => ({
      id: id as keyof typeof database.schema.tables,
      name: table.name,
      columns: Object.keys(table.columns),
    }))
  }),
  getDatabaseTableRows: protectedRoute
    .input(z.enum([DATABASE_FIRST_TABLE_ID!, ...DATABASE_OTHER_TABLE_IDS]))
    .query(({ context: { sig }, input: tableName }) => {
      const database = sig.reactor.use(Database)

      return database.tables[tableName].selectAll()
    }),
  subscribeToLedgerAccount: protectedRoute
    .input(z.string())
    .subscription(({ context: { sig }, input: path }) => {
      const ledgerStore = sig.reactor.use(LedgerStore)
      const postedTransfersTopic = sig.reactor.use(PostedTransfersTopic)
      const pendingTransfersTopic = sig.reactor.use(PendingTransfersTopic)
      const voidedTransfersTopic = sig.reactor.use(VoidedTransfersTopic)

      const account = ledgerStore.getAccount(path as AccountPath)

      if (!account) {
        throw new Error("Account does not exist")
      }

      return createSubscription<LedgerAccount | Transfer>((onData) => {
        const listener = (transfer: Transfer) => {
          if (
            transfer.creditAccount === path ||
            transfer.debitAccount === path
          ) {
            onData(transfer)
          }
        }

        onData(account)

        postedTransfersTopic.on(sig, listener)
        pendingTransfersTopic.on(sig, listener)
        voidedTransfersTopic.on(sig, listener)

        return () => {
          postedTransfersTopic.off(listener)
          pendingTransfersTopic.off(listener)
          voidedTransfersTopic.off(listener)
        }
      })
    }),
})
