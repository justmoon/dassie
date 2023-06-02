import type { Promisable } from "type-fest"

import { isObject } from "@dassie/lib-type-utils"

import type { Actor } from "./actor"
import { createDebugTools } from "./debug/debug-tools"
import { DisposableLifecycle } from "./internal/lifecycle"

/**
 * The reactor will automatically set this property on each instantiated object.
 *
 * @remarks
 *
 * This is a bit of a hack, but it allows our users' code to be cleaner. Specifically, they don't have to repeat the name of the topic, they can just do this and the name `myTopic` will automatically be captured:
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
 * Can be used to add a function that will be automatically called each time a new reference to a context value is created.
 */
export const UseSymbol = Symbol("das:reactive:use")

/**
 * Can be used to add a function that will be automatically called each time a reference to a context value is destroyed.
 */
export const DisposeSymbol = Symbol("das:reactive:dispose")

export type Disposer = () => void
export type AsyncDisposer = () => Promisable<void>

export interface UseOptions {
  /**
   * Custom lifecycle scope to use for this state.
   *
   * @internal
   */
  parentLifecycleScope?: DisposableLifecycle | undefined

  /**
   * If true, the factory will be instantiated fresh every time and will not be stored in the context.
   */
  stateless?: boolean | undefined
}

export type Factory<TInstance> = (reactor: Reactor) => TInstance

export interface ContextState extends WeakMap<Factory<unknown>, unknown> {
  get<T>(key: Factory<T>): T | undefined
  set<T>(key: Factory<T>, value: T): this
}

export class Reactor extends DisposableLifecycle {
  static current: Reactor | undefined

  private contextState = new WeakMap() as ContextState

  constructor() {
    super("Reactor")
  }

  /**
   * Retrieve a value from the reactor's global context. The key is a factory which returns the value sought. If the value does not exist yet, it will be created by running the factory function.
   *
   * @param factory - A function that will be executed to create the value if it does not yet exist in this reactor.
   * @returns The value stored in the context.
   */
  use = <TReturn>(
    factory: Factory<TReturn>,
    { parentLifecycleScope, stateless }: UseOptions = {}
  ): TReturn => {
    let result!: TReturn

    // We use has() to check if the actor is already in the context. Note that the factory's return value may be undefined, so it would not be sufficient to check if the return value of get() is undefined.
    if (stateless || !this.contextState.has(factory)) {
      Reactor.current = this
      result = factory(this)
      Reactor.current = undefined

      if (parentLifecycleScope) {
        parentLifecycleScope.onCleanup(() => {
          this.delete(factory)
        })
      }

      // Tag with factory name
      {
        const target: unknown = result
        if (isObject(target) && FactoryNameSymbol in target) {
          target[FactoryNameSymbol] = factory.name
        }
      }

      // Run intialization function if there is one
      if (
        isObject(result) &&
        InitSymbol in result &&
        typeof result[InitSymbol] === "function"
      ) {
        result[InitSymbol](this)
      }

      if (!stateless) {
        this.contextState.set(factory, result)
      }

      this.debug?.notifyOfContextInstantiation(factory, result)
    } else {
      result = this.contextState.get(factory)!
    }

    if (
      isObject(result) &&
      UseSymbol in result &&
      typeof result[UseSymbol] === "function"
    ) {
      result[UseSymbol](this)
    }

    return result
  }

  /**
   * Access an element in the context but without creating it if it does not yet exist.
   *
   * @param factory - Key to the element in the context.
   * @returns The value stored in the context if any.
   */
  peek = <TReturn>(factory: Factory<TReturn>): TReturn | undefined => {
    return this.contextState.get(factory)
  }

  delete = (factory: Factory<unknown>) => {
    if (!this.contextState.has(factory)) return

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
  set = <T>(factory: Factory<T>, value: T) => {
    this.contextState.set(factory, value)
  }

  /**
   * Returns a set of debug tools for this reactor. Note that this is only available during development.
   */
  debug = createDebugTools(this, this.contextState)
}

export const createReactor = (
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  rootActor?: Factory<Actor<Promisable<void>>> | undefined
): Reactor => {
  const reactor: Reactor = new Reactor()

  if (rootActor) {
    Promise.resolve(reactor.use(rootActor).run(reactor)).catch(
      (error: unknown) => {
        console.error("error running root actor", {
          actor: rootActor.name,
          error,
        })
      }
    )
  }

  return reactor
}
