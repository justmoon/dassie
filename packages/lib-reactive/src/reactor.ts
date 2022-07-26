import type { AsyncOrSync } from "ts-essentials"

import { createLogger } from "@xen-ilp/lib-logger"
import { isObject } from "@xen-ilp/lib-type-utils"

import { createDebugTools } from "./debug/debug-tools"
import { Effect, runEffect } from "./effect"
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

export type Disposer = () => void
export type AsyncDisposer = () => AsyncOrSync<void>

const tagWithEffectName = (target: unknown, effectName: string) => {
  if (isObject(target) && FactoryNameSymbol in target) {
    target[FactoryNameSymbol] = effectName
  }
}

export interface ContextState extends Map<Effect<never>, unknown> {
  get<TReturn>(key: Effect<never, TReturn>): TReturn | undefined
  set<TReturn>(key: Effect<never, TReturn>, value: TReturn): this
}

export interface ContextAccessor {
  <TReturn>(effect: Effect<undefined, TReturn>): TReturn
  <TProperties, TReturn>(
    effect: Effect<TProperties, TReturn>,
    properties: TProperties
  ): TReturn
}

export class Reactor extends LifecycleScope {
  private contextState: ContextState = new Map()

  /**
   * Retrieve a value from the reactor's global context. The key is an effect which returns the value sought. If the value does not exist yet, it will be created by running the effect as a global effect with the same lifetime as the reactor.
   *
   * @param effect - An effect that will be executed to create the value if it does not yet exist in this reactor.
   * @returns The value stored in the context.
   */
  useContext: ContextAccessor = <TProperties, TReturn>(
    effect: Effect<TProperties | undefined, TReturn>,
    properties?: TProperties
  ): TReturn => {
    let result!: TReturn

    // We use has() to check if the effect is already in the context. Note that the effect's result may be undefined, so it would not be sufficient to check if the return value of get() is undefined.
    if (!this.contextState.has(effect)) {
      // Based on the overloaded method signature, TypeScript will enforce that the properties argument is only undefined if the effect's TProperties extends undefined.
      runEffect(
        this,
        effect,
        properties!,
        this,
        (_result) => (result = _result)
      ).catch((error: unknown) => {
        logger.error("error in global effect", { effect: effect.name, error })
      })

      this.setContext(effect, result)

      this.debug?.notifyOfInstantiation(effect)
    } else {
      result = this.contextState.get(effect)!
    }

    return result
  }

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
  setContext = <TReturn>(effect: Effect<never, TReturn>, value: TReturn) => {
    tagWithEffectName(value, effect.name)
    this.contextState.set(effect, value)
  }

  /**
   * Returns a set of debug tools for this reactor. Note that this is only available during development.
   */
  debug = createDebugTools(this.useContext, this.contextState)
}

export const createReactor = (rootEffect: Effect): Reactor => {
  const reactor: Reactor = new Reactor()

  reactor.useContext(rootEffect)

  return reactor
}
