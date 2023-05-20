import type { Promisable } from "type-fest"

import { isObject } from "@dassie/lib-type-utils"

import type { Actor } from "./actor"
import { ActorContext } from "./context"
import { createDebugTools } from "./debug/debug-tools"
import { DisposableLifecycle } from "./internal/lifecycle"
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

export interface RunOptions {
  /**
   * Object with additional debug information that will be merged into the debug log data related to the actor.
   */
  additionalDebugData?: Record<string, unknown> | undefined

  /**
   * Custom lifecycle scope to use for this actor.
   *
   * @internal
   */
  parentLifecycleScope?: DisposableLifecycle | undefined

  /**
   * A string that will be used to prefix the debug log messages related to this actor.
   */
  pathPrefix?: string | undefined

  /**
   * If true, a reference to the actor will be stored in the context.
   */
  register?: boolean | undefined
}

export type Factory<TInstance> = (reactor: Reactor) => TInstance

export interface ContextState extends WeakMap<Factory<unknown>, unknown> {
  get<T>(key: Factory<T>): T | undefined
  set<T>(key: Factory<T>, value: T): this
}

interface RunSignature {
  <TReturn>(factory: Factory<Actor<TReturn>>): Actor<TReturn>
  <TReturn, TProperties>(
    factory: Factory<Actor<TReturn, TProperties>>,
    properties: TProperties,
    options?: RunOptions | undefined
  ): Actor<TReturn, TProperties>
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
   * Execute an actor and return its first return value.
   *
   * @remarks
   *
   * You may pass a second parameter (usually a properties object) which will be passed as the second argument to the actor's behavior function.
   *
   * The actor will run for the duration of the `parentLifecycleScope` or the reactor's lifecycle scope if none is provided.
   *
   * @param factory - A function which returns a new actor instance.
   * @returns The value stored in the context.
   */
  run: RunSignature = <TReturn, TProperties>(
    factory: Factory<Actor<TReturn, TProperties | undefined>>,
    properties?: TProperties,
    {
      additionalDebugData,
      parentLifecycleScope,
      pathPrefix,
      register,
    }: RunOptions = {}
  ): Actor<TReturn, TProperties> => {
    const actor = register ? this.use(factory) : factory(this)

    actor[FactoryNameSymbol] = factory.name

    if (actor.isRunning) {
      throw new Error(`actor is already running: ${actor[FactoryNameSymbol]}`)
    }

    const actorPath = pathPrefix
      ? `${pathPrefix}${actor[FactoryNameSymbol]}`
      : actor[FactoryNameSymbol]

    Object.defineProperty(actor.behavior, "name", {
      value: actorPath,
      writable: false,
    })

    void loopActor(
      this,
      actor,
      actorPath,
      properties,
      parentLifecycleScope ?? this,
      additionalDebugData
    )

    return actor
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
    Promise.resolve(reactor.run(rootActor)).catch((error: unknown) => {
      console.error("error running root actor", {
        actor: rootActor.name,
        error,
      })
    })
  }

  return reactor
}

const loopActor = async <TReturn, TProperties>(
  reactor: Reactor,
  actor: Actor<TReturn, TProperties>,
  actorPath: string,
  properties: TProperties,
  parentLifecycle: DisposableLifecycle,
  additionalDebugData: Record<string, unknown> | undefined
) => {
  for (;;) {
    if (parentLifecycle.isDisposed) return

    const lifecycle = parentLifecycle.deriveChildLifecycle(actorPath)
    const waker = makePromise()

    const context = new ActorContext(
      reactor,
      lifecycle,
      waker.resolve,
      actor[FactoryNameSymbol],
      actorPath
    )

    try {
      actor.isRunning = true

      const actorReturn = actor.behavior(context, properties)

      // Synchronously store the result of the actor's behavior function
      actor.result = actorReturn

      // Wait in case the actor is asynchronous.
      // eslint-disable-next-line @typescript-eslint/await-thenable
      const actorResult = await actorReturn

      actor.write(actorResult)
      actor.promise.resolve(actorResult)

      await waker
    } catch (error: unknown) {
      actor.result = Promise.reject(error)

      console.error("error in actor", {
        actor: actor[FactoryNameSymbol],
        path: actorPath,
        error,
        ...additionalDebugData,
      })
      return
    } finally {
      actor.isRunning = false
      actor.write(undefined)
      actor.promise = makePromise()
      await lifecycle.dispose()
    }
  }
}
