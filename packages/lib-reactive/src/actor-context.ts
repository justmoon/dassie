import { Promisable } from "type-fest"

import { type Actor, type RunOptions } from "./actor"
import { FactoryNameSymbol } from "./internal/context-base"
import { Listener } from "./internal/emit-to-listener"
import { type ReactiveSource } from "./internal/reactive"
import {
  ScheduledTask,
  TaskDescriptor,
  createScheduledTask,
} from "./internal/scheduled-task"
import { WrappedCallback, wrapCallback } from "./internal/wrap-callback"
import {
  DisposableLifecycleScopeImplementation,
  LifecycleScope,
} from "./lifecycle"
import { Mapped } from "./mapped"
import { ReactiveContextImplementation } from "./reactive-context"
import { Reactor } from "./reactor"
import type { ReadonlyTopic, TopicSymbol } from "./topic"
import { Time } from "./types/base-modules/time"
import { ExecutionContext } from "./types/execution-context"
import { Factory } from "./types/factory"
import {
  DisposableLifecycleContext,
  LifecycleContext,
  LifecycleContextShortcuts,
} from "./types/lifecycle-context"
import { ReactiveContext } from "./types/reactive-context"
import { StatefulContext } from "./types/stateful-context"

export const defaultSelector = <T>(value: T) => value

export type ActorReference = Omit<
  Actor<unknown>,
  typeof TopicSymbol | "on" | "once" | "off"
>

export const ActorContextSymbol = Symbol("das:reactive:actor-context")

export interface ActorContext<TBase extends object = object>
  extends LifecycleContext,
    LifecycleContextShortcuts,
    StatefulContext<TBase>,
    ExecutionContext,
    ReactiveContext<TBase> {
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
    topicFactory:
      | Factory<ReadonlyTopic<TMessage>, TBase>
      | ReadonlyTopic<TMessage>,
  ): void

  /**
   * Like {@link ReadonlyTopic.on} but will automatically manage disposing the subscription when the current actor is disposed.
   *
   * @param topic - Reference to the topic, i.e. the message factory function.
   * @param listener - A function that will be called every time a message is emitted on the topic.
   */
  on<TMessage>(
    topicFactory:
      | Factory<ReadonlyTopic<TMessage>, TBase>
      | ReadonlyTopic<TMessage>,
    listener: Listener<TMessage>,
  ): void

  /**
   * Like {@link ReadonlyTopic.once} but will automatically manage disposing the subscription when the current actor is disposed.
   *
   * @param topic - Reference to the topic, i.e. the message factory function.
   * @param listener - A function that will be called every time a message is emitted on the topic.
   */
  once<TMessage>(
    topicFactory:
      | Factory<ReadonlyTopic<TMessage>, TBase>
      | ReadonlyTopic<TMessage>,
    listener: Listener<TMessage>,
  ): void

  /**
   * Create a JS interval that will be automatically cancelled when the current actor is disposed.
   */
  interval(
    callback: () => Promisable<void>,
    intervalInMilliseconds?: number | undefined,
  ): void

  /**
   * Create a JS timeout that will be automatically cancelled when the current actor is disposed.
   */
  timeout(
    callback: () => Promisable<void>,
    delayInMilliseconds?: number | undefined,
  ): void

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
  task(this: ActorContext<{ time: Time }>, task: TaskDescriptor): ScheduledTask

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
   * In all cases, the actor will inherit the current actor's lifecycle, i.e. it will be disposed when the current actor is disposed.
   *
   * @param factory - Factory function of the actor to be instantiated.
   * @returns - Return value of the first invocation of the actor.
   */
  run<TReturn>(
    factory: Factory<Actor<TReturn, TBase>, TBase> | Actor<TReturn, TBase>,
    options?: RunOptions | undefined,
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
    factory:
      | Factory<Mapped<unknown, Actor<TReturn, TBase>>, TBase>
      | Mapped<unknown, Actor<TReturn, TBase>>,
  ): (TReturn | undefined)[]

  /**
   * Run a map of actors sequentially.
   *
   * @remarks
   *
   * This method is similar to {@link runMap}, but it will wait for each actor to finish starting up before starting the next one. When all actors are started, the method returns.
   */
  runMapSequential<TReturn>(
    factory:
      | Factory<Mapped<unknown, Actor<TReturn, TBase>>, TBase>
      | Mapped<unknown, Actor<TReturn, TBase>>,
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

    readonly lifecycle: LifecycleScope,

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
    return this.lifecycle.isDisposed
  }

  get onCleanup() {
    return this.lifecycle.onCleanup
  }

  get offCleanup() {
    return this.lifecycle.offCleanup
  }

  subscribe<TMessage>(
    topicFactory:
      | Factory<ReadonlyTopic<TMessage>, TBase>
      | ReadonlyTopic<TMessage>,
  ) {
    this.once(topicFactory, this.actor.forceRestart)
  }

  on<TMessage>(
    topicFactory:
      | Factory<ReadonlyTopic<TMessage>, TBase>
      | ReadonlyTopic<TMessage>,
    listener: Listener<TMessage>,
  ) {
    const topic =
      typeof topicFactory === "function"
        ? this.reactor.use(topicFactory)
        : topicFactory
    topic.on(this, listener)
  }

  once<TMessage>(
    topicFactory:
      | Factory<ReadonlyTopic<TMessage>, TBase>
      | ReadonlyTopic<TMessage>,
    listener: Listener<TMessage>,
  ) {
    const topic =
      typeof topicFactory === "function"
        ? this.reactor.use(topicFactory)
        : topicFactory
    topic.once(this, listener)
  }

  interval(
    callback: () => Promisable<void>,
    intervalInMilliseconds?: number | undefined,
  ) {
    if (this.isDisposed) return

    const interval = setInterval(
      this.callback(callback),
      intervalInMilliseconds,
    )
    this.onCleanup(() => clearInterval(interval))
  }

  timeout(
    callback: () => Promisable<void>,
    delayInMilliseconds?: number | undefined,
  ): void {
    if (this.isDisposed) return

    const timer = setTimeout(this.callback(callback), delayInMilliseconds)
    this.onCleanup(() => clearTimeout(timer))
  }

  task(
    this: ActorContext<{ time: Time }>,
    descriptor: TaskDescriptor,
  ): ScheduledTask {
    return createScheduledTask(this, descriptor, this.reactor.base.time)
  }

  run<TReturn>(
    actorFactory: Factory<Actor<TReturn, TBase>, TBase> | Actor<TReturn, TBase>,
    options?: RunOptions | undefined,
  ): TReturn | undefined {
    const actor =
      typeof actorFactory === "function"
        ? this.reactor.use(actorFactory)
        : actorFactory

    return actor.run(this, {
      pathPrefix: `${this.path}.`,
      ...options,
    })
  }

  private runMappedItem<TReturn>(
    actor: Actor<TReturn, TBase>,
    mapItemLifecycle: LifecycleScope,
    runOptions: RunOptions,
  ): TReturn | undefined {
    if (this.isDisposed) return

    const actorLifecycle: DisposableLifecycleContext & StatefulContext<TBase> =
      {
        lifecycle: new DisposableLifecycleScopeImplementation(""),
        reactor: this.reactor,
      }
    actorLifecycle.lifecycle.confineTo(mapItemLifecycle)
    actorLifecycle.lifecycle.confineTo(this.lifecycle)
    this.reactor.debug?.tagIntermediateActorScope(actorLifecycle, this)
    return actor.run(actorLifecycle, runOptions)
  }

  runMap<TReturn>(
    factory:
      | Factory<Mapped<unknown, Actor<TReturn, TBase>>, TBase>
      | Mapped<unknown, Actor<TReturn, TBase>>,
  ): (TReturn | undefined)[] {
    const mapped =
      typeof factory === "function" ? this.reactor.use(factory) : factory
    const results: (TReturn | undefined)[] = Array.from({ length: mapped.size })

    const runOptions: RunOptions = {
      pathPrefix: `${this.path}.`,
    }

    let index = 0
    for (const [, actor, mapLifecycle] of mapped) {
      results[index++] = this.runMappedItem(actor, mapLifecycle, runOptions)
    }

    mapped.additions.on(this, ([, actor, mapItemLifecycle]) => {
      this.runMappedItem(actor, mapItemLifecycle, runOptions)
    })

    return results
  }

  async runMapSequential<TReturn>(
    factory:
      | Factory<Mapped<unknown, Actor<TReturn, TBase>>, TBase>
      | Mapped<unknown, Actor<TReturn, TBase>>,
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
    for (const [, actor, mapItemLifecycle] of mapped) {
      // eslint-disable-next-line @typescript-eslint/await-thenable
      results[index++] = await this.runMappedItem(
        actor,
        mapItemLifecycle,
        runOptions,
      )
    }

    mapped.additions.on(this, ([, actor, mapItemLifecycle]) => {
      linearizationPromise = linearizationPromise.then(async () => {
        // eslint-disable-next-line @typescript-eslint/await-thenable
        await this.runMappedItem(actor, mapItemLifecycle, runOptions)
      })
    })

    return results
  }

  withBase<TNewBase extends object>(newBase: TNewBase) {
    return new ActorContextImplementation(
      this.actor,
      this.lifecycle,
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
