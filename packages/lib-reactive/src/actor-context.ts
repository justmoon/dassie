import type { Promisable } from "type-fest"

import type { Actor, RunOptions } from "./actor"
import type { Cancellable } from "./cancellation"
import { FactoryNameSymbol } from "./internal/context-base"
import type { Listener } from "./internal/emit-to-listener"
import { type ReactiveSource } from "./internal/reactive"
import {
  type ScheduledTask,
  type TaskDescriptor,
  createScheduledTask,
} from "./internal/scheduled-task"
import { type WrappedCallback, wrapCallback } from "./internal/wrap-callback"
import type { Mapped } from "./mapped"
import { ReactiveContextImplementation } from "./reactive-context"
import type { Reactor } from "./reactor"
import {
  DisposableScopeImplementation,
  type Scope,
  confineScope,
} from "./scope"
import { type ReadonlySignal, SignalSymbol } from "./signal"
import type { ReadonlyTopic, TopicSymbol } from "./topic"
import type { Clock } from "./types/base-modules/clock"
import type { CancelableContext } from "./types/cancelable-context"
import type { ExecutionContext } from "./types/execution-context"
import type { FactoryOrInstance } from "./types/factory"
import type { ReactiveContext } from "./types/reactive-context"
import type {
  DisposableScopeContext,
  ScopeContext,
  ScopeContextShortcuts,
} from "./types/scope-context"
import type { StatefulContext } from "./types/stateful-context"

export type ActorReference = Omit<
  Actor<unknown>,
  typeof TopicSymbol | "on" | "once" | "off"
>

export const ActorContextSymbol = Symbol("das:reactive:actor-context")

export interface ActorContext<TBase extends object = object>
  extends ScopeContext,
    ScopeContextShortcuts,
    StatefulContext<TBase>,
    ExecutionContext,
    ReactiveContext<TBase>,
    CancelableContext {
  [ActorContextSymbol]: true

  base: TBase

  /**
   * Re-run this actor if a message is emitted on the given topic.
   *
   * @remarks
   *
   * Note that the actor will not necessarily re-run once per event. If multiple events are emitted on the topic before the actor has re-run, it will only re-run once.
   *
   * @param topic - Reference to the topic's factory function.
   */
  subscribe<TMessage>(
    topicFactory: FactoryOrInstance<
      ReadonlyTopic<TMessage> | ReadonlySignal<TMessage>,
      TBase
    >,
  ): void

  /**
   * Like {@link ReadonlyTopic.on} but will automatically manage disposing the subscription when the current actor is disposed.
   *
   * @param topic - Reference to the topic, i.e. the message factory function.
   * @param listener - A function that will be called every time a message is emitted on the topic.
   */
  on<TMessage>(
    topicFactory: FactoryOrInstance<
      ReadonlyTopic<TMessage> | ReadonlySignal<TMessage>,
      TBase
    >,
    listener: Listener<TMessage>,
  ): void

  /**
   * Like {@link ReadonlyTopic.once} but will automatically manage disposing the subscription when the current actor is disposed.
   *
   * @param topic - Reference to the topic, i.e. the message factory function.
   * @param listener - A function that will be called every time a message is emitted on the topic.
   */
  once<TMessage>(
    topicFactory: FactoryOrInstance<
      ReadonlyTopic<TMessage> | ReadonlySignal<TMessage>,
      TBase
    >,
    listener: Listener<TMessage>,
  ): void

  /**
   * Create a JS interval that will be automatically cancelled when the current actor is disposed.
   */
  interval(
    callback: () => Promisable<void>,
    intervalInMilliseconds?: number,
  ): void

  /**
   * Create a JS timeout that will be automatically cancelled when the current actor is disposed.
   */
  timeout(callback: () => Promisable<void>, delayInMilliseconds?: number): void

  /**
   * Create a recurring task.
   *
   * @remarks
   *
   * Please note that the task will not be automatically scheduled. You need to call `task.schedule()` to start it.
   *
   * @param this - The current ActorContext must have a time module in the base.
   * @param task - An object describing the task to be scheduled.
   */
  task(
    this: ActorContext<{ clock: Clock }>,
    task: TaskDescriptor,
  ): ScheduledTask

  /**
   * Instantiate an actor as a child of the current actor.
   *
   * @remarks
   *
   * This is essentially a shorthand for `sig.reactor.use(actorFactory).run(sig.reactor, sig)`. If there are any properties passed, the actor will be instantiated statelessly, i.e. it will not be added to the context and can't be accessed via `sig.reactor.use`.
   *
   * If you want to create a stateless actor but retain a reference to it, you can use `sig.reactor.use(actorFactory, { stateless: true })`.
   *
   * If you want to pass properties to an actor, but still register it in the global context, you can use `sig.reactor.use(actorFactory).run(sig, properties)`.
   *
   * In all cases, the actor will inherit the current actor's scope, i.e. it will be disposed when the current actor is disposed.
   *
   * @param factory - Factory function of the actor to be instantiated.
   * @returns - Return value of the first invocation of the actor.
   */
  run<TReturn>(
    factory: FactoryOrInstance<Actor<TReturn, TBase>, TBase>,
    options?: RunOptions,
  ): TReturn | undefined

  /**
   * Run a map of actors.
   *
   * @remarks
   *
   * The mapped types allows you to specify a map of actors, where the keys are arbitrary strings and the values are actors. The actors will be instantiated as children of the current actor, and will automatically be created and disposed as map items are added or removed.
   * @param factory - Factory function of the mapped actors.
   */
  runMap<TReturn>(
    factory: FactoryOrInstance<Mapped<unknown, Actor<TReturn, TBase>>, TBase>,
  ): (TReturn | undefined)[]

  /**
   * Run a map of actors sequentially.
   *
   * @remarks
   *
   * This method is similar to {@link runMap}, but it will wait for each actor to finish starting up before starting the next one. When all actors are started, the method returns.
   */
  runMapSequential<TReturn>(
    factory: FactoryOrInstance<Mapped<unknown, Actor<TReturn, TBase>>, TBase>,
  ): Promise<(Awaited<TReturn> | undefined)[]>

  /**
   * Wake function. If called, the actor will be cleaned up and re-run.
   */
  forceRestart(this: void): void

  /**
   * Add something to the base context and return an augmented context.
   */
  withBase<TNewBase extends object>(
    base: TNewBase,
  ): ActorContext<TBase & TNewBase>
}

export class ActorContextImplementation<TBase extends object = object>
  extends ReactiveContextImplementation<TBase>
  implements ActorContext
{
  [ActorContextSymbol] = true as const

  constructor(
    /**
     * The actor that this context belongs to.
     */
    private readonly actor: {
      [FactoryNameSymbol]: string
      forceRestart: () => void
      readAndTrack: <TState>(signal: ReactiveSource<TState>) => TState
    },

    readonly scope: Scope,
    readonly cancellable: Cancellable,

    /**
     * Full path of the actor.
     *
     * @remarks
     *
     * The actor path is generated by concatenating the name of the actor with the names of all parent actors.
     */
    readonly path: string,

    reactor: Reactor<TBase>,
  ) {
    super(reactor, actor.readAndTrack)
  }

  get forceRestart() {
    return this.actor.forceRestart
  }

  get base() {
    return this.reactor.base
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

  get abortSignal() {
    return this.cancellable.abortSignal
  }

  subscribe<TMessage>(
    topicFactory: FactoryOrInstance<
      ReadonlyTopic<TMessage> | ReadonlySignal<TMessage>,
      TBase
    >,
  ) {
    this.once(topicFactory, this.actor.forceRestart)
  }

  on<TMessage>(
    topicFactory: FactoryOrInstance<
      ReadonlyTopic<TMessage> | ReadonlySignal<TMessage>,
      TBase
    >,
    listener: Listener<TMessage>,
  ) {
    const topic =
      typeof topicFactory === "function" ?
        this.reactor.use(topicFactory)
      : topicFactory

    ;(SignalSymbol in topic ? topic.values : topic).on(this, listener)
  }

  once<TMessage>(
    topicFactory: FactoryOrInstance<
      ReadonlyTopic<TMessage> | ReadonlySignal<TMessage>,
      TBase
    >,
    listener: Listener<TMessage>,
  ) {
    const topic =
      typeof topicFactory === "function" ?
        this.reactor.use(topicFactory)
      : topicFactory
    ;(SignalSymbol in topic ? topic.values : topic).once(this, listener)
  }

  interval(callback: () => Promisable<void>, intervalInMilliseconds?: number) {
    if (this.isDisposed) return

    const interval = setInterval(
      this.callback(callback),
      intervalInMilliseconds,
    )
    this.onCleanup(() => {
      clearInterval(interval)
    })
  }

  timeout(
    callback: () => Promisable<void>,
    delayInMilliseconds?: number,
  ): void {
    if (this.isDisposed) return

    const timer = setTimeout(this.callback(callback), delayInMilliseconds)
    this.onCleanup(() => {
      clearTimeout(timer)
    })
  }

  task(
    this: ActorContext<{ clock: Clock }>,
    descriptor: TaskDescriptor,
  ): ScheduledTask {
    return createScheduledTask(this, descriptor, this.reactor.base.clock)
  }

  run<TReturn>(
    actorFactory: FactoryOrInstance<Actor<TReturn, TBase>, TBase>,
    options?: RunOptions,
  ): TReturn | undefined {
    const actor =
      typeof actorFactory === "function" ?
        this.reactor.use(actorFactory)
      : actorFactory

    return actor.run(this, {
      pathPrefix: `${this.path}.`,
      ...options,
    })
  }

  private runMappedItem<TReturn>(
    actor: Actor<TReturn, TBase>,
    mapItemScope: Scope,
    runOptions: RunOptions,
  ): TReturn | undefined {
    if (this.isDisposed) return

    const actorScope: DisposableScopeContext & StatefulContext<TBase> = {
      scope: new DisposableScopeImplementation(""),
      reactor: this.reactor,
    }
    confineScope(actorScope.scope, mapItemScope)
    confineScope(actorScope.scope, this.scope)
    this.reactor.debug?.tagIntermediateActorScope(actorScope, this)
    return actor.run(actorScope, runOptions)
  }

  runMap<TReturn>(
    factory: FactoryOrInstance<Mapped<unknown, Actor<TReturn, TBase>>, TBase>,
  ): (TReturn | undefined)[] {
    const mapped =
      typeof factory === "function" ? this.reactor.use(factory) : factory
    const results: (TReturn | undefined)[] = Array.from({ length: mapped.size })

    const runOptions: RunOptions = {
      pathPrefix: `${this.path}.`,
    }

    let index = 0
    for (const [, actor, mapScope] of mapped) {
      results[index++] = this.runMappedItem(actor, mapScope, runOptions)
    }

    mapped.additions.on(this, ([, actor, mapItemScope]) => {
      this.runMappedItem(actor, mapItemScope, runOptions)
    })

    return results
  }

  async runMapSequential<TReturn>(
    factory: FactoryOrInstance<Mapped<unknown, Actor<TReturn, TBase>>, TBase>,
  ): Promise<(Awaited<TReturn> | undefined)[]> {
    const mapped =
      typeof factory === "function" ? this.reactor.use(factory) : factory
    const results: (Awaited<TReturn> | undefined)[] = Array.from({
      length: mapped.size,
    })
    let linearizationPromise = Promise.resolve()

    const runOptions: RunOptions = {
      pathPrefix: `${this.path}.`,
    }

    let index = 0
    for (const [, actor, mapItemScope] of mapped) {
      // eslint-disable-next-line @typescript-eslint/await-thenable
      results[index++] = await this.runMappedItem(
        actor,
        mapItemScope,
        runOptions,
      )
    }

    mapped.additions.on(this, ([, actor, mapItemScope]) => {
      linearizationPromise = linearizationPromise.then(async () => {
        // eslint-disable-next-line @typescript-eslint/await-thenable
        await this.runMappedItem(actor, mapItemScope, runOptions)
      })
    })

    return results
  }

  withBase<TNewBase extends object>(newBase: TNewBase) {
    return new ActorContextImplementation(
      this.actor,
      this.scope,
      this.cancellable,
      this.path,
      this.reactor.withBase(newBase),
    )
  }

  callback<TCallback extends (...parameters: unknown[]) => unknown>(
    callback: TCallback,
  ): WrappedCallback<TCallback> {
    return wrapCallback(callback, this, this.path)
  }
}
