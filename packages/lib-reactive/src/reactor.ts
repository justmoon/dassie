import type { Promisable } from "type-fest"

import { isObject } from "@dassie/lib-type-utils"

import { createDebugTools } from "./debug/debug-tools"
import { Effect, runEffect } from "./effect"
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
export const FactoryNameSymbol = Symbol("das:reactive:factory-name")

/**
 * Can be used to add a function that will be automatically called after a context value has been instantiated.
 */
export const InitSymbol = Symbol("das:reactive:init")

/**
 * Can be used to add a function that will be automatically called after a context value has been disposed.
 */
export const DisposeSymbol = Symbol("das:reactive:dispose")

export type Factory<T> = () => T
export type Disposer = () => void
export type AsyncDisposer = () => Promisable<void>

const tagWithEffectName = (target: unknown, effectName: string) => {
  if (isObject(target) && FactoryNameSymbol in target) {
    target[FactoryNameSymbol] = effectName
  }
}

export interface ContextState extends Map<Factory<unknown>, unknown> {
  get<T>(key: Factory<T>): T | undefined
  set<T>(key: Factory<T>, value: T): this
}

export class Reactor extends LifecycleScope {
  private contextState: ContextState = new Map()
  private contextReferenceCount = new Map<Factory<unknown>, number>()

  /**
   * Retrieve a value from the reactor's global context. The key is a factory which returns the value sought. If the value does not exist yet, it will be created by running the factory function.
   *
   * @param factory - A function that will be executed to create the value if it does not yet exist in this reactor.
   * @returns The value stored in the context.
   */
  useContext = <TReturn>(factory: Factory<TReturn>): TReturn => {
    let result: TReturn

    // We use has() to check if the effect is already in the context. Note that the factory's return value may be undefined, so it would not be sufficient to check if the return value of get() is undefined.
    if (!this.contextState.has(factory)) {
      result = factory()

      // Run intialization function if there is one
      if (
        isObject(result) &&
        InitSymbol in result &&
        typeof result[InitSymbol] === "function"
      ) {
        result[InitSymbol](this)
      }

      this.setContext(factory, result)

      this.debug?.notifyOfInstantiation(factory, result)
    } else {
      result = this.contextState.get(factory)!

      // As an optimization, we only track reference counts of two or higher.
      // When the object exists in the context map, but not in the reference count map, it means that the reference count is 1.
      const referenceCount = this.contextReferenceCount.get(factory) ?? 1
      this.contextReferenceCount.set(factory, referenceCount + 1)
    }

    return result
  }

  /**
   * Access an element in the context but without creating it if it does not yet exist. This also does not increase the reference count.
   *
   * @param factory - Key to the element in the context.
   * @returns The value stored in the context if any.
   */
  peekContext = <TReturn>(factory: Factory<TReturn>): TReturn | undefined => {
    return this.contextState.get(factory)
  }

  disposeContext = (factory: Factory<unknown>) => {
    if (!this.contextState.has(factory)) return

    const referenceCount = this.contextReferenceCount.get(factory) ?? 1

    if (referenceCount > 2) {
      this.contextReferenceCount.set(factory, referenceCount - 1)
      return
    } else if (referenceCount === 2) {
      this.contextReferenceCount.delete(factory)
      return
    }

    const result = this.contextState.get(factory)

    // Call the value's dispose function if there is one
    if (
      isObject(result) &&
      DisposeSymbol in result &&
      typeof result[DisposeSymbol] === "function"
    ) {
      result[DisposeSymbol](this)
    }

    this.contextState.delete(factory)
  }

  /**
   * Manually set the instance of a given element in the reactor's global context.
   *
   * @remarks
   *
   * This is not something you are likely to need to use. It is used internally to set values on the context. It could be useful for testing/mocking purposes.
   *
   * @param factory - The factory which should be used as the key to store this context element.
   * @param value - The value to store in the context.
   */
  setContext = <T>(factory: Factory<T>, value: T) => {
    tagWithEffectName(value, factory.name)
    this.contextState.set(factory, value)
  }

  /**
   * Returns a set of debug tools for this reactor. Note that this is only available during development.
   */
  debug = createDebugTools(this.useContext, this.contextState)
}

export const createReactor = (rootEffect: Effect): Reactor => {
  const reactor: Reactor = new Reactor()

  runEffect(reactor, rootEffect, undefined, reactor).catch((error: unknown) => {
    console.error("error in root effect", { effect: rootEffect.name, error })
  })

  return reactor
}
