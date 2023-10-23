import type { ConditionalKeys } from "type-fest"

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

export type Behavior<TReturn = unknown, TProperties = unknown> = (
  sig: ActorContext,
  properties: TProperties,
) => TReturn

export const ActorSymbol = Symbol("das:reactive:actor")

export type MessageHandler = (message: never) => unknown

export type MessageHandlerRecord = Record<string, MessageHandler>

export type InferActorMessageType<
  TInstance,
  TMethod extends string & keyof TInstance,
> = TInstance[TMethod] extends (message: infer TMessage) => unknown
  ? TMessage
  : never

export type InferActorReturnType<
  TInstance,
  TMethod extends string & keyof TInstance,
> = TInstance[TMethod] extends (...parameters: never[]) => infer TReturn
  ? TReturn
  : never

export type Actor<TInstance, TProperties = undefined> = ReactiveObserver &
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
     * The function that initializes the actor. Used internally. You should call reactor.run() or sig.run() if you want to start the actor.
     */
    behavior: Behavior<TInstance, TProperties>

    /**
     * Run the actor and return the result.
     *
     * @remarks
     *
     * @param lifecycle - The parent lifecycle scope. The actor will automatically be disposed when this scope is disposed.
     * @param properties - The properties to pass to the actor.
     * @returns The return value of the actor.
     */
    run: RunSignature<TInstance, TProperties>

    /**
     * Fire-and-forget message to the actor. The message will be delivered asynchronously and any return value will be ignored.
     *
     * @param message - The message to send to the actor.
     */
    tell: <
      TMethod extends string &
        ConditionalKeys<Awaited<TInstance>, MessageHandler>,
    >(
      this: void,
      method: TMethod,
      message: InferActorMessageType<Awaited<TInstance>, TMethod>,
    ) => void

    /**
     * Asynchronously message the actor and receive a response as a promise.
     *
     * @param message - The message to send to the actor.
     * @returns A promise that will resolve with the response from the actor.
     */
    ask: <
      TMethod extends string &
        ConditionalKeys<Awaited<TInstance>, MessageHandler>,
    >(
      this: void,
      method: TMethod,
      message: InferActorMessageType<Awaited<TInstance>, TMethod>,
    ) => Promise<InferActorReturnType<Awaited<TInstance>, TMethod>>
  }

interface RunSignature<TReturn, TProperties> {
  (reactor: Reactor, lifecycle: LifecycleScope): TReturn | undefined
  (
    reactor: Reactor,
    lifecycle: LifecycleScope,
    properties: TProperties,
    options?: RunOptions | undefined,
  ): TReturn | undefined
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

class ActorImplementation<TInstance, TProperties>
  extends ContextBase
  implements
    Omit<Actor<TInstance, TProperties>, keyof ReadonlyTopic>,
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

  constructor(public behavior: Behavior<TInstance, TProperties>) {
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
    properties?: TProperties,
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

    void this.loop(
      reactor,
      actorPath,
      properties!,
      lifecycle,
      additionalDebugData,
    )

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
    properties: TProperties,
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
      context.attachToParent(parentLifecycle)
      context.onCleanup(resetActor)

      try {
        this.isRunning = true

        const previousSources = this.sources
        this.sources = new Set()
        this.setCacheStatus(CacheStatus.Clean)

        // Assigning the behavior function to a local variable prevents
        // "ActorImplementation." from cluttering the stack trace.
        const behavior = this.behavior

        const actorReturn = behavior(context, properties)

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

  tell: Actor<TInstance, TProperties>["tell"] = (method, message) => {
    this.ask(method, message).catch((error: unknown) => {
      console.error("error in actor handler", {
        error,
        actor: this[FactoryNameSymbol],
        method,
      })
    })
  }

  ask: Actor<TInstance, TProperties>["ask"] = async (method, message) => {
    if (import.meta.env.DEV && !this.isRunning) {
      debugOptimisticActorCall(this.promise, this[FactoryNameSymbol], method)
    }

    const actorInstance = await this.promise

    return (
      actorInstance[method] as (
        parameter: typeof message,
      ) => InferActorReturnType<Awaited<TInstance>, typeof method>
    )(message)
  }

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

type CreateActorSignature = <TInstance, TProperties = undefined>(
  behavior: Behavior<TInstance, TProperties>,
) => Actor<TInstance, TProperties>

export const createActor: CreateActorSignature = <
  TInstance,
  TProperties = undefined,
>(
  behavior: Behavior<TInstance, TProperties>,
): Actor<TInstance, TProperties> => {
  const actor = new ActorImplementation(behavior)
  return Object.assign(actor, createReactiveTopic(actor))
}

export const isActor = (
  object: unknown,
): object is Actor<MessageHandlerRecord, unknown> =>
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
