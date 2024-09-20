import type { Promisable } from "type-fest"

import { isObject } from "@dassie/lib-type-utils"

import type { Actor } from "./actor"
import { DebugTools, createDebugTools } from "./debug/debug-tools"
import {
  DisposeSymbol,
  FactoryNameSymbol,
  InitSymbol,
  UseSymbol,
} from "./internal/context-base"
import { type WrappedCallback, wrapCallback } from "./internal/wrap-callback"
import { type DisposableScope, createScope } from "./scope"
import type { ExecutionContext } from "./types/execution-context"
import type { Factory, FactoryOrInstance } from "./types/factory"
import type {
  DisposableScopeContext,
  DisposableScopeContextShortcuts,
} from "./types/scope-context"
import type { StatefulContext } from "./types/stateful-context"

export interface ContextState
  extends WeakMap<Factory<unknown, never>, unknown> {
  get<T>(key: Factory<T, never>): T | undefined
  set<T>(key: Factory<T, never>, value: T): this
}

export interface UseOptions {
  /**
   * If true, the factory will be instantiated fresh every time and will not be stored in the context.
   */
  stateless?: boolean | undefined
}

export interface Reactor<TBase extends object = object>
  extends StatefulContext<TBase>,
    ExecutionContext,
    DisposableScopeContext,
    DisposableScopeContextShortcuts {
  readonly base: TBase

  /**
   * Retrieve a value from the context or create it if it does not yet exist.
   *
   * @param factory - A function that is used both as the key to the value in the context and as the factory function to create the value if it does not yet exist in the context.
   * @param options - Options for this use call.
   */
  use<TReturn>(
    factory: Factory<TReturn, TBase>,
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

  /**
   * Add something to the base context and return a new reactor which shares the state but has the new base.
   */
  withBase<TNewBase extends object>(base: TNewBase): Reactor<TBase & TNewBase>

  debug: ReturnType<typeof createDebugTools>
}

class ReactorImplementation<TBase extends object = object> implements Reactor {
  public readonly reactor = this

  public readonly debug: DebugTools | undefined

  constructor(
    readonly base: TBase,
    readonly contextState: ContextState,
    readonly scope: DisposableScope,
    debug?: DebugTools | undefined,
  ) {
    this.debug = debug ?? createDebugTools(this, contextState)
  }

  get isDisposed() {
    return this.scope.isDisposed
  }

  get onCleanup() {
    return this.scope.onCleanup
  }

  get offCleanup() {
    return this.scope.offCleanup
  }

  get dispose() {
    return this.scope.dispose
  }

  use<TReturn>(
    factory: Factory<TReturn, TBase>,
    { stateless }: UseOptions = {},
  ): TReturn {
    let result!: TReturn

    // We use has() to check if the actor is already in the context. Note that the factory's return value may be undefined, so it would not be sufficient to check if the return value of get() is undefined.
    if (stateless || !this.contextState.has(factory)) {
      result = factory(this)

      // Tag with factory name in debug mode
      if (this.debug && isObject(result)) {
        Object.defineProperty(result, FactoryNameSymbol, {
          value: factory.name,
          enumerable: false,
          writable: true,
        })
      }

      // Run initialization function if there is one
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

  delete(factory: Factory<unknown, never>) {
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

  withBase<TNewBase extends object>(
    newBase: TNewBase,
  ): Reactor<TBase & TNewBase> {
    return new ReactorImplementation(
      { ...this.base, ...newBase },
      this.contextState,
      this.scope,
      this.debug,
    )
  }

  callback<TCallback extends (...parameters: unknown[]) => unknown>(
    callback: TCallback,
  ): WrappedCallback<TCallback> {
    return wrapCallback(callback, this, "Reactor")
  }
}

interface CreateReactor {
  (rootActorFactory?: FactoryOrInstance<Actor<Promisable<void>>>): Reactor
  <TBase extends object>(
    rootActorFactory:
      | FactoryOrInstance<Actor<Promisable<void>, TBase>>
      | undefined,
    base: TBase,
  ): Reactor<TBase>
}

export const createReactor: CreateReactor = <TBase extends object>(
  rootActorFactory?:
    | FactoryOrInstance<Actor<Promisable<void>, TBase>>
    | undefined,
  base?: TBase,
): Reactor<TBase> => {
  const reactor: Reactor<TBase> = new ReactorImplementation(
    base ?? ({} as TBase),
    new WeakMap() as ContextState,
    createScope("Reactor"),
  )

  if (rootActorFactory) {
    const rootActor =
      typeof rootActorFactory === "function" ?
        reactor.use(rootActorFactory)
      : rootActorFactory
    Promise.resolve(rootActor.run(reactor)).catch((error: unknown) => {
      console.error("error running root actor", {
        actor: rootActor[FactoryNameSymbol],
        error,
      })
    })
  }

  return reactor
}
