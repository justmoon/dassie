import type { Promisable } from "type-fest"

import { isObject } from "@dassie/lib-type-utils"

import { DisposableLifecycleScope, createLifecycleScope } from "."
import type { Actor } from "./actor"
import { createDebugTools } from "./debug/debug-tools"
import {
  DisposeSymbol,
  FactoryNameSymbol,
  InitSymbol,
  UseSymbol,
} from "./internal/context-base"

export type Disposer = () => void
export type AsyncDisposer = () => Promisable<void>

export interface UseOptions {
  /**
   * Custom lifecycle scope to use for this state.
   *
   * @internal
   */
  parentLifecycleScope?: DisposableLifecycleScope | undefined

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

export interface Reactor {
  /**
   * Retrieve a value from the context or create it if it does not yet exist.
   *
   * @param factory - A function that is used both as the key to the value in the context and as the factory function to create the value if it does not yet exist in the context.
   * @param options - Options for this use call.
   */
  use<TReturn>(
    factory: Factory<TReturn>,
    options?: UseOptions | undefined,
  ): TReturn

  /**
   * Access an element in the context but without creating it if it does not yet exist.
   *
   * @param factory - Key to the element in the context.
   * @returns The value stored in the context if any.
   */
  peek<TReturn>(factory: Factory<TReturn>): TReturn | undefined

  /**
   * Delete an element from the context.
   *
   * @param factory - The factory which should be used as the key of the element to delete.
   */
  delete(factory: Factory<unknown>): void

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
  set<T>(factory: Factory<T>, value: T): void

  debug: ReturnType<typeof createDebugTools>

  lifecycle: DisposableLifecycleScope
}

class ReactorImplementation implements Reactor {
  public readonly lifecycle = createLifecycleScope("Reactor")
  private readonly contextState = new WeakMap() as ContextState

  public readonly debug = createDebugTools(this, this.contextState)

  use<TReturn>(
    factory: Factory<TReturn>,
    { parentLifecycleScope, stateless }: UseOptions = {},
  ): TReturn {
    let result!: TReturn

    // We use has() to check if the actor is already in the context. Note that the factory's return value may be undefined, so it would not be sufficient to check if the return value of get() is undefined.
    if (stateless || !this.contextState.has(factory)) {
      result = factory(this)

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

      this.debug?.notifyOfContextInstantiation(result)
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

  peek<TReturn>(factory: Factory<TReturn>): TReturn | undefined {
    return this.contextState.get(factory)
  }

  delete(factory: Factory<unknown>) {
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

  set<T>(factory: Factory<T>, value: T) {
    this.contextState.set(factory, value)
  }
}

export const createReactor = (
  rootActor?: Factory<Actor<Promisable<void>>> | undefined,
): Reactor => {
  const reactor: Reactor = new ReactorImplementation()

  if (rootActor) {
    Promise.resolve(
      reactor.use(rootActor).run(reactor, reactor.lifecycle),
    ).catch((error: unknown) => {
      console.error("error running root actor", {
        actor: rootActor.name,
        error,
      })
    })
  }

  return reactor
}
