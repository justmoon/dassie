import type { ConditionalPick, SetReturnType } from "type-fest"

import { isObject } from "@dassie/lib-type-utils"

import { ActorContext } from "./actor-context"
import { ContextBase, FactoryNameSymbol } from "./internal/context-base"
import { makePromise } from "./internal/promise"
import {
  CacheStatus,
  CacheUninitialized,
  ReactiveObserver,
  ReactiveSource,
} from "./internal/reactive"
import { createReactiveTopic } from "./internal/reactive-topic"
import { DisposableLifecycleScope, LifecycleScope } from "./lifecycle"
import { Reactor } from "./reactor"
import { ReadonlySignal, Reducer, SignalSymbol } from "./signal"
import { ReadonlyTopic } from "./topic"

export type Behavior<TReturn = unknown> = (sig: ActorContext) => TReturn

export const ActorSymbol = Symbol("das:reactive:actor")

// eslint-disable-next-line @typescript-eslint/no-explicit-any
export type ActorApiHandler = (...parameters: any[]) => unknown

export interface ActorApiProxy<TCallback extends ActorApiHandler> {
  /**
   * Fire-and-forget message to the actor. The message will be delivered asynchronously and any return value will be ignored.
   *
   * @param message - The message to send to the actor.
   */
  tell(...parameters: Parameters<TCallback>): void

  /**
   * Asynchronously message the actor and receive a response as a promise.
   *
   * @param message - The message to send to the actor.
   * @returns A promise that will resolve with the response from the actor.
   */
  ask: ReturnType<TCallback> extends PromiseLike<unknown>
    ? TCallback
    : SetReturnType<TCallback, Promise<ReturnType<TCallback>>>
}

export type InferActorApi<TInstance> = {
  [K in keyof ConditionalPick<
    TInstance,
    ActorApiHandler
  >]: TInstance[K] extends ActorApiHandler ? ActorApiProxy<TInstance[K]> : never
}

class ApiProxy {
  constructor(
    private readonly actor: Pick<
      Actor<unknown>,
      "isRunning" | "promise" | typeof FactoryNameSymbol
    >,
    private readonly method: string,
  ) {}

  private async getInstance() {
    if (import.meta.env.DEV && !this.actor.isRunning) {
      debugOptimisticActorCall(
        this.actor.promise,
        this.actor[FactoryNameSymbol],
        this.method,
      )
    }

    const actorInstance = (await this.actor.promise) as Record<
      string,
      (...parameters: unknown[]) => unknown
    >
    return actorInstance
  }

  ask = async (...parameters: unknown[]) => {
    const actorInstance = await this.getInstance()

    const handler = actorInstance[this.method]

    if (!handler) {
      throw new Error(`Actor does not expose method: ${this.method}`)
    }

    return handler(...parameters)
  }

  tell = (...parameters: unknown[]) => {
    this.ask(...parameters).catch((error: unknown) => {
      console.error("error in actor handler", {
        error,
        actor: this.actor[FactoryNameSymbol],
        method: this.method,
      })
    })
  }
}

export type Actor<TInstance> = ReactiveObserver &
  ReadonlySignal<Awaited<TInstance> | undefined> & {
    /**
     * Marks this object as a value.
     */
    [ActorSymbol]: true

    /**
     * Whether the actor is currently instantiated inside of a reactor.
     */
    readonly isRunning: boolean

    /**
     * The result of the most recent succesful execution of the actor.
     *
     * Note that this is the raw result which may be a promise. If you want to get the resolved value, use `actor.read()`.
     *
     * If the actor hasn't run yet, the result will be undefined.
     * If the actor throws an error, the result will be a rejected promise which is represented by a Promise<never> type.
     */
    readonly result: TInstance | undefined

    /**
     * A promise which will resolve with the result of the actor.
     *
     * @remarks
     *
     * If the actor is not currently running, this promise will resolve the next
     * time the actor completes executing its behavior function.
     */
    readonly promise: Promise<TInstance>

    /**
     * Run the actor and return the result.
     *
     * @remarks
     *
     * @param lifecycle - The parent lifecycle scope. The actor will automatically be disposed when this scope is disposed.
     * @returns The return value of the actor.
     */
    run: (
      reactor: Reactor,
      lifecycle: LifecycleScope,
      options?: RunOptions | undefined,
    ) => TInstance | undefined

    /**
     * If the actor exposes a public API, you can interact with it here.
     *
     * @example
     *
     * ```ts
     * myActor.api.myMethod.tell({ foo: "bar" })
     * ```
     */
    api: InferActorApi<Awaited<TInstance>>
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
  parentLifecycleScope?: DisposableLifecycleScope | undefined

  /**
   * A string that will be used to prefix the debug log messages related to this actor.
   */
  pathPrefix?: string | undefined

  /**
   * An alternative name to use for the Actor in the debug log messages.
   */
  overrideName?: string | undefined
}

class ActorImplementation<TInstance>
  extends ContextBase
  implements
    Omit<Actor<TInstance>, keyof ReadonlyTopic>,
    ReactiveObserver,
    ReactiveSource<Awaited<TInstance> | undefined>
{
  [SignalSymbol] = true as const;
  [ActorSymbol] = true as const

  isRunning = false
  result: TInstance | undefined
  promise = makePromise<TInstance>()
  private waker = makePromise()

  private cache: Awaited<TInstance> | undefined
  private cacheStatus: CacheStatus = CacheStatus.Clean
  private observers = new Set<ReactiveObserver>()
  private sources = new Set<ReactiveSource<unknown>>()
  private isReadingSource = false

  constructor(public behavior: Behavior<TInstance>) {
    super()
  }

  read(): Awaited<TInstance> | undefined {
    return this.cache
  }

  stale(newCacheStatus: CacheStatus) {
    if (!this.isReadingSource && this.cacheStatus < newCacheStatus) {
      if (this.cacheStatus === CacheStatus.Clean) {
        this.waker.resolve()
      }

      this.cacheStatus = newCacheStatus
    }
  }

  recompute() {}

  run(
    reactor: Reactor,
    lifecycle: LifecycleScope,
    { additionalDebugData, pathPrefix, overrideName }: RunOptions = {},
  ) {
    if (this.isRunning) {
      throw new Error(`actor is already running: ${this[FactoryNameSymbol]}`)
    }

    const actorName = overrideName ?? this[FactoryNameSymbol] ?? "Anonymous"
    const actorPath = pathPrefix ? `${pathPrefix}${actorName}` : actorName

    Object.defineProperty(this.behavior, "name", {
      value: actorName,
      writable: false,
    })

    void this.loop(reactor, actorPath, lifecycle, additionalDebugData)

    return this.result
  }

  private async reset() {
    await this.promise
    this.isRunning = false
    this.cache = undefined
    this.promise = makePromise()
  }

  private setCacheStatus(newCacheStatus: CacheStatus) {
    this.cacheStatus = newCacheStatus
  }

  private async loop(
    reactor: Reactor,
    actorPath: string,
    parentLifecycle: LifecycleScope,
    additionalDebugData: Record<string, unknown> | undefined,
  ) {
    const resetActor = this.reset.bind(this)

    this.waker = makePromise()

    for (;;) {
      if (parentLifecycle.isDisposed) return

      const context = new ActorContext(
        this[FactoryNameSymbol],
        actorPath,
        reactor,
        () => {
          this.cacheStatus = CacheStatus.Dirty
          this.waker.resolve()
        },
        (signal) => {
          this.isReadingSource = true
          const value = signal.read()
          this.isReadingSource = false
          this.sources.add(signal)
          signal.addObserver(this)
          return value
        },
      )
      context.confineTo(parentLifecycle)
      context.onCleanup(resetActor)

      try {
        this.isRunning = true

        const previousSources = this.sources
        this.sources = new Set()
        this.setCacheStatus(CacheStatus.Clean)

        // Assigning the behavior function to a local variable prevents
        // "ActorImplementation." from cluttering the stack trace.
        const behavior = this.behavior

        const actorReturn = behavior(context)

        // Synchronously store the result of the actor's behavior function
        this.result = actorReturn

        // Wait in case the actor is asynchronous.
        // eslint-disable-next-line @typescript-eslint/await-thenable
        const actorResult = await actorReturn

        if (this.cache !== actorResult) {
          if (this.observers.size > 0) {
            for (const observer of this.observers) {
              observer.stale(CacheStatus.Dirty)
            }
          }
          this.cache = actorResult
        }
        this.promise.resolve(actorResult)

        for (const source of previousSources) {
          if (!this.sources.has(source)) {
            source.removeObserver(this)
          }
        }

        do {
          await this.waker

          if (this.cacheStatus === CacheStatus.Check) {
            for (const source of this.sources) {
              source.read()
            }
          }

          // If any of the sources had changed, they would have marked us dirty
          if (this.cacheStatus === CacheStatus.Check) {
            this.setCacheStatus(CacheStatus.Clean)
          }

          this.waker = makePromise()
        } while (this.cacheStatus !== CacheStatus.Dirty)
      } catch (error: unknown) {
        console.error("error in actor", {
          actor: this[FactoryNameSymbol],
          path: actorPath,
          error,
          ...additionalDebugData,
        })
        return
      } finally {
        await context.dispose()
      }
    }
  }

  api = new Proxy(this, {
    get(target, method) {
      if (typeof method !== "string") {
        throw new TypeError("Actor API method names must be strings")
      }

      return new ApiProxy(target, method)
    },
  }) as unknown as InferActorApi<Awaited<TInstance>>

  /**
   * Apply a reducer which will accept the current state and return a new state.
   *
   * @param reducer - The reducer to apply to the state.
   * @returns The new state of the signal.
   */
  update(reducer: Reducer<Awaited<TInstance> | undefined>) {
    const newValue = reducer(
      this.cache === CacheUninitialized ? undefined : this.cache,
    )
    this.cache = newValue
    return newValue
  }

  public addObserver(observer: ReactiveObserver) {
    this.observers.add(observer)
  }

  public removeObserver(observer: ReactiveObserver) {
    this.observers.delete(observer)
  }
}

export const createActor = <TInstance>(
  behavior: Behavior<TInstance>,
): Actor<TInstance> => {
  const actor = new ActorImplementation(behavior)
  return Object.assign(actor, createReactiveTopic(actor))
}

export const isActor = (object: unknown): object is Actor<unknown> =>
  isObject(object) && object[ActorSymbol] === true

const DEBUG_ACTOR_OPTIMISTIC_CALL_TIMEOUT = 2000
const debugOptimisticActorCall = (
  actorPromise: Promise<unknown>,
  actorName: string,
  method: string,
) => {
  const timeout = setTimeout(() => {
    console.error("Actor called but never instantiated", {
      actor: actorName,
      method,
    })
  }, DEBUG_ACTOR_OPTIMISTIC_CALL_TIMEOUT)

  void actorPromise.finally(() => clearTimeout(timeout))
}
