import type { Promisable } from "type-fest"

import { isObject } from "@dassie/lib-type-utils"

import type { ActorFactory, Behavior } from "./actor"
import { ActorContext } from "./context"
import { createDebugTools } from "./debug/debug-tools"
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

export interface UseOptions {
  /**
   * Custom lifecycle scope to use for this state.
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

export interface RunOptions<TReturn> {
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
  register?: boolean | undefined
}

export type Factory<TInstance> = (reactor: Reactor) => TInstance

export interface ContextState
  extends WeakMap<Factory<unknown> | Behavior, unknown> {
  get<T>(key: Factory<T> | Behavior<T>): T | undefined
  set<T>(key: Factory<T> | Behavior<T>, value: T): this
}

interface RunSignature {
  <TReturn, TInitialState>(
    factory: ActorFactory<TReturn, undefined, TInitialState>
  ): TReturn
  <TProperties, TReturn, TInitialState>(
    factory: ActorFactory<TReturn, TProperties, TInitialState>,
    properties: TProperties,
    options?: RunOptions<TReturn> | undefined
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
  use = <TReturn>(
    factory: (reactor: Reactor) => TReturn,
    { parentLifecycleScope, pathPrefix, stateless }: UseOptions = {}
  ) => {
    let result!: TReturn

    // We use has() to check if the effect is already in the context. Note that the factory's return value may be undefined, so it would not be sufficient to check if the return value of get() is undefined.
    if (stateless || !this.contextState.has(factory)) {
      const factoryPath = pathPrefix
        ? `${pathPrefix}${factory.name}`
        : factory.name

      result = factory(this)
      if (parentLifecycleScope) {
        parentLifecycleScope.onCleanup(() => {
          this.delete(factory)
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
   * Instantiate an actor in the reactor's global context. The key is a factory which returns the value sought. If the value does not exist yet, it will be created by running the factory function.
   *
   * @param effect - A function that will be executed to create the value if it does not yet exist in this reactor.
   * @returns The value stored in the context.
   */
  run: RunSignature = <TProperties, TReturn, TInitialState>(
    factory: ActorFactory<TReturn, TProperties | undefined, TInitialState>,
    properties?: TProperties,
    {
      onResult,
      additionalDebugData,
      parentLifecycleScope,
      pathPrefix,
      register,
    }: RunOptions<TReturn> = {}
  ) => {
    let result: TReturn | undefined

    const actor = register ? this.use(factory) : factory(this)

    const actorPath = pathPrefix ? `${pathPrefix}${factory.name}` : factory.name

    loopEffect(
      this,
      actor.behavior,
      actorPath,
      properties,
      parentLifecycleScope ?? this,
      (_result) => {
        result = _result
        if (register) {
          void Promise.resolve(_result).then((resolvedResult) => {
            actor.write(resolvedResult)
          })
        }
        if (onResult && _result !== undefined) {
          onResult(_result)
        }
      },
      () => {
        if (register) {
          actor.write(actor.initialState)
        }
      }
    ).catch((error: unknown) => {
      console.error("error in actor", {
        effect: factory.name,
        path: actorPath,
        error,
        ...additionalDebugData,
      })
    })

    return result
  }

  /**
   * Access an element in the context but without creating it if it does not yet exist. This also does not increase the reference count.
   *
   * @param factory - Key to the element in the context.
   * @returns The value stored in the context if any.
   */
  peek = <TReturn>(
    factory: Factory<TReturn> | Behavior<TReturn>
  ): TReturn | undefined => {
    return this.contextState.get(factory)
  }

  delete = (factory: Factory<unknown> | Behavior) => {
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
  set = <T>(factory: Factory<T> | Behavior<T>, value: T) => {
    this.contextState.set(factory, value)
  }

  /**
   * Returns a set of debug tools for this reactor. Note that this is only available during development.
   */
  debug = createDebugTools(this.use, this.contextState)
}

export const createReactor = (
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  rootEffect?: ActorFactory<Promisable<void>> | undefined
): Reactor => {
  const reactor: Reactor = new Reactor()

  if (rootEffect) {
    Promise.resolve(reactor.run(rootEffect)).catch((error: unknown) => {
      console.error("error running root actor", {
        effect: rootEffect.name,
        error,
      })
    })
  }

  return reactor
}

const loopEffect = async <TProperties, TReturn>(
  reactor: Reactor,
  effect: Behavior<TReturn, TProperties>,
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

    const context = new ActorContext(
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
      disposeCallback()
      await lifecycle.dispose()
    }
  }
}
