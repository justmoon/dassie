import type { Promisable } from "type-fest"

import { isObject } from "@dassie/lib-type-utils"

import { createDebugTools } from "./debug/debug-tools"
import { Effect, EffectContext } from "./effect"
import { LifecycleScope } from "./internal/lifecycle-scope"
import { makePromise } from "./internal/promise"

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

const tagWithEffectName = (target: unknown, effectName: string) => {
  if (isObject(target) && FactoryNameSymbol in target) {
    target[FactoryNameSymbol] = effectName
  }
}

export interface UseOptions<TReturn> {
  /**
   * A callback which will be called with the effect's return value each time it executes.
   */
  onResult?: ((result: TReturn) => void) | undefined

  /**
   * Object with additional debug information that will be merged into the debug log data related to the effect.
   */
  additionalDebugData?: Record<string, unknown> | undefined

  /**
   * Custom lifecycle scope to use for this effect.
   *
   * @internal
   */
  parentLifecycleScope?: LifecycleScope | undefined

  /**
   * A string that will be used to prefix the debug log messages related to this effect.
   */
  pathPrefix?: string | undefined

  /**
   * If true, the factory or effect will be instantiated fresh every time and will not be stored in the context.
   */
  stateless?: boolean | undefined
}

export interface ContextState extends WeakMap<Effect<never>, unknown> {
  get<T>(key: Effect<never, T>): T | undefined
  set<T>(key: Effect<never, T>, value: T): this
}

interface UseSignature {
  <TReturn>(factory: Effect<undefined, TReturn>): TReturn
  <TProperties, TReturn>(
    factory: Effect<TProperties, TReturn>,
    properties: TProperties,
    options?: UseOptions<TReturn> | undefined
  ): TReturn
}

export class Reactor extends LifecycleScope {
  private contextState = new WeakMap() as ContextState

  /**
   * Retrieve a value from the reactor's global context. The key is a factory which returns the value sought. If the value does not exist yet, it will be created by running the factory function.
   *
   * @param factory - A function that will be executed to create the value if it does not yet exist in this reactor.
   * @returns The value stored in the context.
   */
  use: UseSignature = <TProperties, TReturn>(
    factory: Effect<TProperties | undefined, TReturn>,
    properties?: TProperties,
    options?: UseOptions<TReturn> | undefined
  ) => {
    let result!: TReturn

    // We use has() to check if the effect is already in the context. Note that the factory's return value may be undefined, so it would not be sufficient to check if the return value of get() is undefined.
    if (options?.stateless || !this.contextState.has(factory)) {
      const factoryPath = options?.pathPrefix
        ? `${options.pathPrefix}${factory.name}`
        : factory.name

      if (factory.length === 0) {
        result = (factory as () => TReturn)()
        if (options?.onResult) {
          options.onResult(result)
        }
        if (options?.parentLifecycleScope) {
          options.parentLifecycleScope.onCleanup(() => {
            this.delete(factory)
          })
        }
      } else {
        loopEffect(
          this,
          factory,
          factoryPath,
          properties,
          options?.parentLifecycleScope ?? this,
          (_result) => {
            result = _result
            if (options?.onResult) {
              options.onResult(result)
            }
          },
          () => {
            this.delete(factory)
          }
        ).catch((error: unknown) => {
          console.error("error in effect", {
            effect: factory.name,
            path: factoryPath,
            error,
            ...options?.additionalDebugData,
          })
        })
      }

      tagWithEffectName(result, factoryPath)

      // Run intialization function if there is one
      if (
        isObject(result) &&
        InitSymbol in result &&
        typeof result[InitSymbol] === "function"
      ) {
        result[InitSymbol](this)
      }

      if (!options?.stateless) {
        this.contextState.set(factory, result)
      }

      this.debug?.notifyOfInstantiation(factory, result)
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
   * Access an element in the context but without creating it if it does not yet exist. This also does not increase the reference count.
   *
   * @param factory - Key to the element in the context.
   * @returns The value stored in the context if any.
   */
  peek = <TReturn>(factory: Effect<never, TReturn>): TReturn | undefined => {
    return this.contextState.get(factory)
  }

  delete = (factory: Effect<never>) => {
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
  set = <T>(factory: Effect<never, T>, value: T) => {
    this.contextState.set(factory, value)
  }

  /**
   * Returns a set of debug tools for this reactor. Note that this is only available during development.
   */
  debug = createDebugTools(this.use, this.contextState)
}

export const createReactor = (rootEffect?: Effect | undefined): Reactor => {
  const reactor: Reactor = new Reactor()

  if (rootEffect) {
    reactor.use(rootEffect)
  }

  return reactor
}

const loopEffect = async <TProperties, TReturn>(
  reactor: Reactor,
  effect: Effect<TProperties, TReturn>,
  effectPath: string,
  properties: TProperties,
  parentLifecycle: LifecycleScope,
  resultCallback: (result: TReturn) => void,
  disposeCallback: () => void
) => {
  for (;;) {
    if (parentLifecycle.isDisposed) return

    const lifecycle = parentLifecycle.deriveChildLifecycle()
    const waker = makePromise()

    const context = new EffectContext(
      reactor,
      lifecycle,
      waker.resolve,
      effect.name,
      effectPath
    )

    try {
      const effectResult = effect(context, properties)

      // runEffect MUST always call the resultCallback or throw an error
      resultCallback(effectResult)

      // --- There must be no `await` before calling the resultCallback ---

      // Wait in case the effect is asynchronous.
      // eslint-disable-next-line @typescript-eslint/await-thenable
      await effectResult

      await waker
    } finally {
      await lifecycle.dispose()
      disposeCallback()
    }
  }
}
