import { Promisable } from "type-fest"

import { isObject } from "@dassie/lib-type-utils"

import { FactoryNameSymbol, Reactor } from "."
import { type Actor, type RunOptions } from "./actor"
import { Listener } from "./internal/emit-to-listener"
import { type ReactiveSource } from "./internal/reactive"
import {
  DisposableLifecycleScope,
  DisposableLifecycleScopeImplementation,
} from "./lifecycle"
import { Mapped } from "./mapped"
import { ReactiveContextImplementation } from "./reactive-context"
import type { ReadonlyTopic } from "./topic"
import { Factory } from "./types/factory"
import { ReactiveContext } from "./types/reactive-context"
import { StatefulContext } from "./types/stateful-context"

export const defaultSelector = <T>(value: T) => value

export interface ActorContext
  extends DisposableLifecycleScope,
    StatefulContext,
    ReactiveContext {
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
    topicFactory: Factory<ReadonlyTopic<TMessage>> | ReadonlyTopic<TMessage>,
  ): void

  /**
   * Like {@link ReadonlyTopic.on} but will automatically manage disposing the subscription when the current actor is disposed.
   *
   * @param topic - Reference to the topic, i.e. the message factory function.
   * @param listener - A function that will be called every time a message is emitted on the topic.
   */
  on<TMessage>(
    topicFactory: Factory<ReadonlyTopic<TMessage>> | ReadonlyTopic<TMessage>,
    listener: Listener<TMessage>,
  ): void

  /**
   * Like {@link ReadonlyTopic.once} but will automatically manage disposing the subscription when the current actor is disposed.
   *
   * @param topic - Reference to the topic, i.e. the message factory function.
   * @param listener - A function that will be called every time a message is emitted on the topic.
   */
  once<TMessage>(
    topicFactory: Factory<ReadonlyTopic<TMessage>> | ReadonlyTopic<TMessage>,
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
    factory: Factory<Actor<TReturn>> | Actor<TReturn>,
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
      | Factory<Mapped<unknown, Actor<TReturn>>>
      | Mapped<unknown, Actor<TReturn>>,
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
      | Factory<Mapped<unknown, Actor<TReturn>>>
      | Mapped<unknown, Actor<TReturn>>,
  ): Promise<(TReturn | undefined)[]>

  /**
   * Wake function. If called, the actor will be cleaned up and re-run.
   */
  readonly wake: () => void
}

export class ActorContextImplementation
  extends ReactiveContextImplementation
  implements ActorContext
{
  constructor(
    /**
     * Name of the actor.
     *
     * @remarks
     *
     * This is automatically derived from the function name.
     */
    name: string,

    /**
     * Full path of the actor.
     *
     * @remarks
     *
     * The actor path is generated by concatenating the name of the actor with the names of all parent actors.
     */
    readonly path: string,

    reactor: Reactor,

    readonly wake: () => void,

    _get: <TState>(signal: ReactiveSource<TState>) => TState,
  ) {
    super(name, reactor, _get)
  }

  subscribe<TMessage>(
    topicFactory: Factory<ReadonlyTopic<TMessage>> | ReadonlyTopic<TMessage>,
  ) {
    this.once(topicFactory, this.wake)
  }

  on<TMessage>(
    topicFactory: Factory<ReadonlyTopic<TMessage>> | ReadonlyTopic<TMessage>,
    listener: Listener<TMessage>,
  ) {
    const topic =
      typeof topicFactory === "function"
        ? this.reactor.use(topicFactory)
        : topicFactory
    topic.on(this, listener)
  }

  once<TMessage>(
    topicFactory: Factory<ReadonlyTopic<TMessage>> | ReadonlyTopic<TMessage>,
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

    const interval = setInterval(() => {
      try {
        const result = callback()

        if (
          isObject(result) &&
          "catch" in result &&
          typeof result["catch"] === "function"
        ) {
          result["catch"]((error: unknown) => {
            console.error("error in async interval callback", {
              actor: this.name,
              path: this.path,
              error,
            })
          })
        }
      } catch (error) {
        console.error("error in interval callback", {
          actor: this.name,
          path: this.path,
          error,
        })
      }
    }, intervalInMilliseconds)
    this.onCleanup(() => clearInterval(interval))
  }

  timeout(
    callback: () => Promisable<void>,
    delayInMilliseconds?: number | undefined,
  ): void {
    if (this.isDisposed) return

    const timer = setTimeout(() => {
      try {
        const result = callback()

        if (
          isObject(result) &&
          "catch" in result &&
          typeof result["catch"] === "function"
        ) {
          result["catch"]((error: unknown) => {
            console.error("error in async timeout callback", {
              actor: this.name,
              path: this.path,
              error,
            })
          })
        }
      } catch (error) {
        console.error("error in timeout callback", {
          actor: this.name,
          path: this.path,
          error,
        })
      }
    }, delayInMilliseconds)
    this.onCleanup(() => clearTimeout(timer))
  }

  run<TReturn>(
    actorFactory: Factory<Actor<TReturn>> | Actor<TReturn>,
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

  runMap<TReturn>(
    factory: Factory<Mapped<unknown, Actor<TReturn>>>,
  ): (TReturn | undefined)[] {
    const mapped = this.reactor.use(factory)
    const results: (TReturn | undefined)[] = Array.from({ length: mapped.size })

    const runOptions: RunOptions = {
      pathPrefix: `${this.path}.`,
      overrideName: `${mapped[FactoryNameSymbol] ?? "anonymous"}[]`,
    }

    let index = 0
    for (const [, actor, mapLifecycle] of mapped) {
      const actorLifecycle: DisposableLifecycleScope & StatefulContext =
        Object.assign(new DisposableLifecycleScopeImplementation(""), {
          reactor: this.reactor,
        })
      actorLifecycle.confineTo(mapLifecycle)
      actorLifecycle.confineTo(this)
      results[index++] = actor.run(actorLifecycle, runOptions)
    }

    mapped.additions.on(this, ([, actor, mapItemLifecycle]) => {
      const actorLifecycle: DisposableLifecycleScope & StatefulContext =
        Object.assign(new DisposableLifecycleScopeImplementation(""), {
          reactor: this.reactor,
        })
      actorLifecycle.confineTo(mapItemLifecycle)
      actorLifecycle.confineTo(this)
      actor.run(actorLifecycle, runOptions)
    })

    return results
  }

  async runMapSequential<TReturn>(
    factory: Factory<Mapped<unknown, Actor<TReturn>>>,
  ): Promise<(TReturn | undefined)[]> {
    const mapped = this.reactor.use(factory)
    const results: (TReturn | undefined)[] = Array.from({ length: mapped.size })
    let linearizationPromise = Promise.resolve()

    const runOptions: RunOptions = {
      pathPrefix: `${this.path}.`,
      overrideName: `${mapped[FactoryNameSymbol] ?? "anonymous"}[]`,
    }

    let index = 0
    for (const [, actor, mapItemLifecycle] of mapped) {
      const actorLifecycle: DisposableLifecycleScope & StatefulContext =
        Object.assign(new DisposableLifecycleScopeImplementation(""), {
          reactor: this.reactor,
        })
      actorLifecycle.confineTo(mapItemLifecycle)
      actorLifecycle.confineTo(this)
      // eslint-disable-next-line @typescript-eslint/await-thenable
      results[index++] = await actor.run(actorLifecycle, runOptions)
    }

    mapped.additions.on(this, ([, actor, mapItemLifecycle]) => {
      linearizationPromise = linearizationPromise.then(async () => {
        const actorLifecycle: DisposableLifecycleScope & StatefulContext =
          Object.assign(new DisposableLifecycleScopeImplementation(""), {
            reactor: this.reactor,
          })
        actorLifecycle.confineTo(mapItemLifecycle)
        actorLifecycle.confineTo(this)
        // eslint-disable-next-line @typescript-eslint/await-thenable
        await actor.run(actorLifecycle, runOptions)
      })
    })

    return results
  }
}
