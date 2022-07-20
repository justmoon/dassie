import type { AsyncOrSync } from "ts-essentials"

import { EffectContext, useRootEffect } from "./effect"
import { DebugTools, createDebugTools } from "./internal/debug-tools"
import { LifecycleScope } from "./internal/lifecycle-scope"

export type Effect<TProperties = unknown, TReturn = unknown> = (
  sig: EffectContext,
  properties: TProperties
) => TReturn
export type Disposer = () => void
export type AsyncDisposer = () => AsyncOrSync<void>

export interface Reactor {
  /**
   * Retrieve a value from the global context of the reactor, indexed by its factory function. If no value is found, one is created via the factory, stored in the context, and returned.
   *
   * @param factory - A function that will be called to create a value if one does not exist in the context.
   * @returns The value stored in the context.
   */
  fromContext: <T>(factory: () => T) => T

  /**
   * Register a cleanup handler for this reactor.
   *
   * @remarks
   *
   * The handler will be called when the reactor is disposed.
   *
   * @param cleanupHandler - A function that will be called when the reactor is disposed.
   */
  onCleanup: (cleanupHandler: () => void) => void

  /**
   * Dispose of the entire reactive system.
   */
  dispose: AsyncDisposer

  /**
   * Returns a set of debug tools for this reactor. Note that this is only available during development and will throw an error if used in production.
   */
  debug: () => DebugTools
}

export interface ContextState extends Map<() => unknown, unknown> {
  get<T>(key: () => T): T | undefined
  set<T>(key: () => T, value: T): this
}

export const createReactor = (rootEffect: Effect): Reactor => {
  const contextState: ContextState = new Map()

  const lifecycle = new LifecycleScope()

  const fromContext = <T>(factory: () => T): T => {
    let value = contextState.get(factory)

    if (!value) {
      value = factory()

      contextState.set(factory, value)
    }

    return value
  }

  const debug = () => createDebugTools(contextState)

  const reactor: Reactor = {
    fromContext,
    onCleanup: lifecycle.onCleanup.bind(lifecycle),
    dispose: lifecycle.dispose.bind(lifecycle),
    debug,
  }

  useRootEffect(reactor, lifecycle, rootEffect)

  return reactor
}
