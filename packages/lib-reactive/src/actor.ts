import type { ConditionalPick, SetReturnType } from "type-fest"

import { isObject } from "@dassie/lib-type-utils"

import { StatefulContext } from "."
import { ActorContext, ActorContextImplementation } from "./actor-context"
import { ContextBase, FactoryNameSymbol } from "./internal/context-base"
import { makePromise } from "./internal/promise"
import {
  CacheStatus,
  CacheUninitialized,
  ReactiveObserver,
  ReactiveSource,
} from "./internal/reactive"
import { createReactiveTopic } from "./internal/reactive-topic"
import { DisposableLifecycleScope, createLifecycleScope } from "./lifecycle"
import { ReadonlySignal, Reducer, SignalSymbol } from "./signal"
import { ReadonlyTopic } from "./topic"
import { LifecycleContext } from "./types/lifecycle-context"

export type Behavior<TReturn = unknown, TBase extends object = object> = (
  sig: ActorContext<TBase>,
) => TReturn

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
    : SetReturnType<TCallback, Promise<Awaited<ReturnType<TCallback>>>>
}

export type InferActorApi<TReturn> = {
  [K in keyof ConditionalPick<
    TReturn,
    ActorApiHandler
  >]: TReturn[K] extends ActorApiHandler ? ActorApiProxy<TReturn[K]> : never
}

class ApiProxy {
  constructor(
    private readonly actor: Pick<
      Actor<unknown>,
      "currentContext" | "promise" | typeof FactoryNameSymbol
    >,
    private readonly method: string,
  ) {}

  private async getInstance() {
    if (import.meta.env.DEV && !this.actor.currentContext) {
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

export interface Actor<TReturn, TBase extends object = object>
  extends ReactiveObserver,
    ReadonlySignal<Awaited<TReturn> | undefined> {
  /**
   * Marks this object as a value.
   */
  [ActorSymbol]: true

  /**
   * A reference to the actor context if the actor is currently running. Otherwise undefined.
   */
  readonly currentContext: ActorContext | undefined

  /**
   * The result of the most recent succesful execution of the actor.
   *
   * Note that this is the raw result which may be a promise. If you want to get the resolved value, use `actor.read()`.
   *
   * If the actor hasn't run yet, the result will be undefined.
   * If the actor throws an error, the result will be a rejected promise which is represented by a Promise<never> type.
   */
  readonly result: TReturn | undefined

  /**
   * A promise which will resolve with the result of the actor.
   *
   * @remarks
   *
   * If the actor is not currently running, this promise will resolve the next
   * time the actor completes executing its behavior function.
   */
  readonly promise: Promise<TReturn>

  /**
   * Run the actor and return the result.
   *
   * @remarks
   *
   * @param lifecycle - The parent lifecycle scope. The actor will automatically be disposed when this scope is disposed.
   * @returns The return value of the actor.
   */
  run: (
    parentContext: StatefulContext<TBase> & LifecycleContext,
    options?: RunOptions | undefined,
  ) => TReturn | undefined

  /**
   * Invalidate the actor and force it to restart at the next opportunity.
   *
   * @remarks
   *
   * Calling this method will indicate to the actor that its dependencies may have changed at it should restart. If
   * the actor is already in the process of restarting, it will restart again once it is done. However, it will not
   * buffer more than one restart.
   */
  forceRestart: () => void

  /**
   * If the actor exposes a public API, you can interact with it here.
   *
   * @example
   *
   * ```ts
   * myActor.api.myMethod.tell({ foo: "bar" })
   * ```
   */
  api: InferActorApi<Awaited<TReturn>>
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

export class ActorImplementation<TReturn, TBase extends object>
  extends ContextBase
  implements
    Omit<Actor<TReturn, TBase>, keyof ReadonlyTopic>,
    ReactiveObserver,
    ReactiveSource<Awaited<TReturn> | undefined>
{
  [SignalSymbol] = true as const;
  [ActorSymbol] = true as const

  currentContext: ActorContext<TBase> | undefined = undefined
  result: TReturn | undefined
  promise = makePromise<TReturn>()
  private waker = makePromise()

  private cache: Awaited<TReturn> | undefined
  private cacheStatus: CacheStatus = CacheStatus.Clean
  private observers = new Set<ReactiveObserver>()
  private sources = new Set<ReactiveSource<unknown>>()
  private isReadingSource = false

  constructor(public behavior: Behavior<TReturn, TBase>) {
    super()
  }

  read(): Awaited<TReturn> | undefined {
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
    parentContext: StatefulContext<TBase> & LifecycleContext,
    { additionalDebugData, pathPrefix, overrideName }: RunOptions = {},
  ) {
    if (this.currentContext) {
      throw new Error(`actor is already running: ${this[FactoryNameSymbol]}`)
    }

    const actorName = overrideName ?? this[FactoryNameSymbol] ?? "Anonymous"
    const actorPath = pathPrefix ? `${pathPrefix}${actorName}` : actorName

    Object.defineProperty(this.behavior, "name", {
      value: actorName,
      writable: false,
    })

    void this.loop(parentContext, actorPath, additionalDebugData)

    return this.result
  }

  private async reset() {
    await this.promise
    this.currentContext = undefined
    this.cache = undefined
    this.promise = makePromise()
  }

  private setCacheStatus(newCacheStatus: CacheStatus) {
    this.cacheStatus = newCacheStatus
  }

  forceRestart = () => {
    this.cacheStatus = CacheStatus.Dirty
    this.waker.resolve()
  }

  /**
   * Read a signal and track it as a dependency of the actor.
   *
   * @remarks
   *
   * Used internally by the actor context to track dependencies.
   *
   * @internal
   */
  readAndTrack = <TState>(signal: ReactiveSource<TState>) => {
    this.isReadingSource = true
    const value = signal.read()
    this.isReadingSource = false
    this.sources.add(signal)
    signal.addObserver(this)
    return value
  }

  private async loop(
    parentContext: StatefulContext<TBase> & LifecycleContext,
    actorPath: string,
    additionalDebugData: Record<string, unknown> | undefined,
  ) {
    const resetActor = this.reset.bind(this)

    this.waker = makePromise()

    for (;;) {
      if (parentContext.lifecycle.isDisposed) return

      const lifecycle = createLifecycleScope(this[FactoryNameSymbol])
      lifecycle.confineTo(parentContext.lifecycle)
      lifecycle.onCleanup(resetActor)

      const context = new ActorContextImplementation(
        this,
        lifecycle,
        actorPath,
        parentContext.reactor,
      )

      parentContext.reactor.debug?.tagActorContext(context, parentContext)

      this.currentContext = context

      try {
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
        await lifecycle.dispose()
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
  }) as unknown as InferActorApi<Awaited<TReturn>>

  /**
   * Apply a reducer which will accept the current state and return a new state.
   *
   * @param reducer - The reducer to apply to the state.
   * @returns The new state of the signal.
   */
  update(reducer: Reducer<Awaited<TReturn> | undefined>) {
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

export const createActor = <TReturn, TBase extends object>(
  behavior: Behavior<TReturn, TBase>,
): Actor<TReturn, TBase> => {
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
