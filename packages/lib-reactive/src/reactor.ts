import type { AsyncOrSync } from "ts-essentials"

import { createLogger } from "@xen-ilp/lib-logger"
import { isObject } from "@xen-ilp/lib-type-utils"

import { DebugTools, createDebugTools } from "./debug/debug-tools"
import { EffectContext, runEffect } from "./effect"
import { LifecycleScope } from "./internal/lifecycle-scope"

const logger = createLogger("xen:reactive:reactor")

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

const tagWithEffectName = (target: unknown, factoryName: string) => {
  if (isObject(target) && FactoryNameSymbol in target) {
    target[FactoryNameSymbol] = factoryName
  }
}

export interface Reactor {
  /**
   * Retrieve a value from the reactor's global context. The key is an effect which returns the value sought. If the value does not exist yet, it will be created by running the effect as a global effect with the same lifetime as the reactor.
   *
   * @param effect - An effect that will be executed to create the value if it does not yet exist in this reactor.
   * @returns The value stored in the context.
   */
  useContext<TReturn>(effect: Effect<undefined, TReturn>): TReturn
  useContext<TProperties, TReturn>(
    effect: Effect<TProperties, TReturn>,
    properties: TProperties
  ): TReturn
  useContext<TProperties, TReturn>(
    effect: Effect<TProperties | undefined, TReturn>,
    properties?: TProperties
  ): TReturn

  /**
   * Manually set the instance of a given element in the reactor's global context.
   *
   * @remarks
   *
   * This is not something you are likely to need to use. It is used internally to set values on the context. It could be useful for testing/mocking purposes.
   *
   * @param effect - The effect which should be used as the key to store this context element.
   * @param value - The value to store in the context.
   */
  setContext: <TReturn>(effect: Effect<never, TReturn>, value: TReturn) => void

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

export interface ContextState extends Map<Effect<never>, unknown> {
  get<TReturn>(key: Effect<never, TReturn>): TReturn | undefined
  set<TReturn>(key: Effect<never, TReturn>, value: TReturn): this
}

export const createReactor = (rootEffect: Effect): Reactor => {
  const contextState: ContextState = new Map()

  const lifecycle = new LifecycleScope()

  const useContext = <TProperties, TReturn>(
    effect: Effect<TProperties, TReturn>,
    properties?: TProperties
  ): TReturn => {
    // We use has() to check if the effect is already in the context. Note that the effect's result may be undefined, so it would not be sufficient to check if the return value of get() is undefined.
    if (!contextState.has(effect)) {
      let result!: TReturn

      // Based on the overloaded method signature, TypeScript will enforce that the properties argument is only undefined if the effect's TProperties extends undefined.
      runEffect(
        reactor,
        effect,
        properties!,
        lifecycle,
        (_result) => (result = _result)
      ).catch((error: unknown) => {
        logger.error("error in global effect", { effect: effect.name, error })
      })

      setContext(effect, result)

      debug?.notifyOfInstantiation(effect)
    }

    // runEffect must always either set the context value or throw an error. If `value` is still undefined here, it's because the effect returned undefined.
    const value = contextState.get(effect)!

    return value
  }

  const setContext = <TReturn>(
    effect: Effect<never, TReturn>,
    value: TReturn
  ) => {
    tagWithEffectName(value, effect.name)
    contextState.set(effect, value)
  }

  const debug = createDebugTools(useContext, contextState)

  const reactor: Reactor = {
    useContext,
    setContext,
    onCleanup: lifecycle.onCleanup.bind(lifecycle),
    dispose: lifecycle.dispose.bind(lifecycle),
    debug,
  }

  useContext(rootEffect)

  return reactor
}
