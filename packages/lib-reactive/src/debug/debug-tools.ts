import { castImmutable } from "immer"

import { isObject } from "@dassie/lib-type-utils"

import { Actor } from "../actor"
import { ActorContextSymbol } from "../actor-context"
import { LifecycleScope } from "../lifecycle"
import { Mapped } from "../mapped"
import type { ContextState, Reactor } from "../reactor"
import { StatefulContext } from "../types/stateful-context"

export interface ContextEntry {
  uniqueId: number
  reference: WeakRef<Record<keyof never, unknown>>
}

export const DebugIdSymbol = Symbol("das:reactive:debug-id")
export const MappedOwnerSymbol = Symbol("das:reactive:mapped-owner")
export const ActorDebugParentContextSymbol = Symbol("das:reactive:debug-parent")
export const ActorIntermediateScopeParent = Symbol(
  "das:reactive:debug-intermediate-scope-parent",
)

const hasIntermediateScopeParent = (
  target: object,
): target is {
  [ActorIntermediateScopeParent]: StatefulContext<object> & LifecycleScope
} => ActorIntermediateScopeParent in target

export class DebugTools {
  private static uniqueId = 0
  private context = new Map<number, ContextEntry>()
  private registry = new FinalizationRegistry<ContextEntry>(
    (value: ContextEntry) => {
      this.context.delete(value.uniqueId)
    },
  )

  constructor(
    readonly reactor: Reactor,
    readonly contextState: ContextState,
  ) {}

  notifyOfContextInstantiation(value: unknown) {
    if (!isObject(value)) {
      return
    }

    const contextEntry = {
      uniqueId: DebugTools.uniqueId++,
      reference: new WeakRef(value),
    }
    this.context.set(contextEntry.uniqueId, contextEntry)
    this.registry.register(value, contextEntry)

    Object.defineProperty(value, DebugIdSymbol, {
      value: contextEntry.uniqueId,
      enumerable: false,
    })
  }

  notifyOfMappedInstantiation(
    value: unknown,
    mapped: Mapped<unknown, unknown>,
  ) {
    if (!isObject(value)) {
      return
    }

    const mappedId = (mapped as object as { [DebugIdSymbol]?: number })[
      DebugIdSymbol
    ]

    if (mappedId === undefined) {
      return
    }

    Object.defineProperty(value, MappedOwnerSymbol, {
      value: mappedId,
      enumerable: false,
    })

    this.notifyOfContextInstantiation(value)
  }

  /**
   * Retrieve the list of all current context entries
   *
   * @returns All currently instantiated values in the reactor.
   */
  getContext() {
    return castImmutable(this.context)
  }

  /**
   * Called internally by the actor to facilitate tracking the actor hierarchy.
   *
   * @param context - The ActorContext that should be tagged with debug information.
   * @param actor - The actor who owns the context.
   * @param parentContext - The parent context this actor is nested in.
   */
  tagActorContext(
    context: { [ActorContextSymbol]: true },
    parentContext: StatefulContext<object> & LifecycleScope,
  ) {
    if (hasIntermediateScopeParent(parentContext)) {
      parentContext = parentContext[ActorIntermediateScopeParent]
    }

    Object.defineProperty(context, ActorDebugParentContextSymbol, {
      value: parentContext,
      enumerable: false,
    })
  }

  /**
   * Called internally by the actor to facilitate tracking the actor hierarchy.
   *
   * @param scope - The intermediate scope that should be tagged with debug information.
   * @param context - The actual actor context that is the parent of the intermediate scope.
   */
  tagIntermediateActorScope(
    scope: LifecycleScope,
    context: { [ActorContextSymbol]: true },
  ) {
    Object.defineProperty(scope, ActorIntermediateScopeParent, {
      value: context,
      enumerable: false,
    })
  }

  getActorParent(actor: Actor<unknown>): Actor<unknown> | undefined {
    // eslint-disable-next-line @typescript-eslint/no-unsafe-return, @typescript-eslint/no-unsafe-member-access, @typescript-eslint/no-explicit-any
    return (actor as any).currentContext?.[ActorDebugParentContextSymbol]?.actor
  }
}

export const createDebugTools = (
  reactor: Reactor,
  topicsCache: ContextState,
) => (import.meta.env.DEV ? new DebugTools(reactor, topicsCache) : undefined)
