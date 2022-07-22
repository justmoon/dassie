import type { AsyncOrSync } from "ts-essentials"

import { isObject } from "@xen-ilp/lib-type-utils"

import { DebugTools, createDebugTools } from "./debug/debug-tools"
import { EffectContext, useRootEffect } from "./effect"
import type { Factory } from "./factory"
import { LifecycleScope } from "./internal/lifecycle-scope"

/**
 * The reactor will automatically set this property on each instantiated object.
 *
 * @remarks
 *
 * This is a bit of a hack, but it allows our users' code to be cleaner. Specifically, they don't have to repeat the name of the topic/store/etc., they can just do this and the name `myTopic` will automatically be captured:
 *
 * @example
 *
 * ```ts
 * const myTopic = () => reactor.createTopic<string>()
 * ```
 */
export const FactoryNameSymbol = Symbol("xen:reactive:factory-name")

export type Effect<TProperties = unknown, TReturn = unknown> = (
  sig: EffectContext,
  properties: TProperties
) => TReturn
export type Disposer = () => void
export type AsyncDisposer = () => AsyncOrSync<void>

const tagWithFactoryName = (target: unknown, factoryName: string) => {
  if (isObject(target) && FactoryNameSymbol in target) {
    target[FactoryNameSymbol] = factoryName
  }
}

export interface Reactor {
  /**
   * Retrieve a value from the global context of the reactor, indexed by its factory function. If no value is found, one is created via the factory, stored in the context, and returned.
   *
   * @param factory - A function that will be called to create a value if one does not exist in the context.
   * @returns The value stored in the context.
   */
  fromContext: <T>(factory: Factory<T>) => T

  /**
   * Register a cleanup handler for this reactor.
   *
   * @remarks
   *
   * The handler will be called when the reactor is disposed.
   *
   * @param cleanupHandler - A function that will be called when the reactor is disposed.
   */
  onCleanup: (cleanupHandler: AsyncDisposer) => void

  /**
   * Dispose of the entire reactive system.
   */
  dispose: AsyncDisposer

  /**
   * Returns a set of debug tools for this reactor. Note that this is only available during development.
   */
  debug: DebugTools | undefined
}

export interface ContextState extends Map<() => unknown, unknown> {
  get<T>(key: Factory<T>): T | undefined
  set<T>(key: Factory<T>, value: T): this
}

export const createReactor = (rootEffect: Effect): Reactor => {
  const contextState: ContextState = new Map()

  const lifecycle = new LifecycleScope()

  const fromContext = <T>(factory: Factory<T>): T => {
    let value = contextState.get(factory)

    if (!value) {
      value = factory(reactor)

      debug?.notifyOfInstantiation(factory, value)
      tagWithFactoryName(value, factory.name)

      contextState.set(factory, value)
    }

    return value
  }

  const debug = createDebugTools(fromContext, contextState)

  const reactor: Reactor = {
    fromContext,
    onCleanup: lifecycle.onCleanup.bind(lifecycle),
    dispose: lifecycle.dispose.bind(lifecycle),
    debug,
  }

  useRootEffect(reactor, lifecycle, rootEffect)

  return reactor
}
