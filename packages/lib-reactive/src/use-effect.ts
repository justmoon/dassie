import { createLogger } from "@xen-ilp/lib-logger"

import type { AsyncDisposer, Effect, Listener, Reactor } from "./create-reactor"
import type { Topic } from "./create-topic"
import { LifecycleScope } from "./internal/lifecycle-scope"
import { Waker } from "./internal/waker"

const logger = createLogger("xen:reactive")

export class EffectContext {
  constructor(
    /**
     * A reference to the current reactor.
     *
     * @remarks
     *
     * If you want to pass something to an external component to allow that component to interact with the reactive system, you can use this reference.
     */
    readonly reactor: Reactor,

    /**
     * An object corresponding to this context's lifetime.
     */
    private readonly lifecycle: LifecycleScope,

    private readonly waker: Waker
  ) {}

  /**
   * Read the current value from a topic and automatically re-run the effect if the value changes.
   *
   * @remarks
   *
   * Tracking is only possible in the synchronous portion of the effect. If get is called after the synchronous execution, e.g. in response to an event or after an await, it will throw an error.
   *
   * To read a value without tracking, use {@link read}.
   *
   * @param topic - Reference to the topic, i.e. the message factory function.
   * @param selector - Used to select only part of the message from a given topic. This can be useful to avoid re-running the effect if only an irrelevant portion of the message has changed.
   * @param comparator - By default, the reactor checks for strict equality (`===`) to determine if the value has changed. This can be overridden by passing a custom comparator function.
   * @returns The most recent value from the topic (or the initial value), narrowed by the selector.
   */
  get<TMessage, TInitial>(
    topic: Topic<TMessage, never, TInitial>
  ): TMessage | TInitial
  get<TMessage, TInitial, TSelection>(
    topic: Topic<TMessage, never, TInitial>,
    selector: (state: TMessage | TInitial) => TSelection,
    comparator?: (oldValue: TSelection, newValue: TSelection) => boolean
  ): TSelection
  get<TMessage, TSelection, TInitial>(
    topic: Topic<TMessage, never, TInitial>,
    selector: (state: TMessage | TInitial) => TSelection = (a) =>
      // Based on the overloaded function signature, the selector parameter may be omitted iff TMessage equals TSelection.
      // Therefore this cast is safe.
      a as unknown as TSelection,
    comparator: (a: TSelection, b: TSelection) => boolean = (a, b) => a === b
  ) {
    const handleTopicMessage = (message: TMessage) => {
      const newValue = selector(message)
      if (!comparator(value, newValue)) {
        // Once we have detected a change we can stop listening because once the effect re-runs it will create a new listener anyways.
        dispose()
        notify()
      }
    }

    const notify = this.waker.notify
    const value = selector(this.reactor.read(topic))
    const dispose = this.reactor.on(topic, handleTopicMessage)
    this.lifecycle.onCleanup(dispose)
    return value
  }

  /**
   * Like {@link get}, but without tracking. That means that calling read will not cause the effect to be re-run if there are new messages on the given topic.
   *
   * @param topic - Reference to the topic, i.e. the message factory function.
   */
  read<TMessage, TTrigger, TInitial>(
    topic: Topic<TMessage, TTrigger, TInitial>
  ): TMessage | TInitial {
    return this.reactor.read(topic)
  }

  /**
   * Like {@link get}, but reruns the effect regardless of whether the value has changed.
   *
   * @param topic - Reference to the topic, i.e. the message factory function.
   */
  subscribe<TMessage, TTrigger, TInitial>(
    topic: Topic<TMessage, TTrigger, TInitial>
  ) {
    this.once(topic, this.waker.notify)
  }

  /**
   * Like {@link Reactor.on} but will automatically manage disposing the subscription when the current effect is disposed.
   *
   * @param topic - Reference to the topic, i.e. the message factory function.
   * @param listener - A function that will be called every time a message is emitted on the topic.
   */
  on<TMessage, TTrigger, TInitial>(
    topic: Topic<TMessage, TTrigger, TInitial>,
    listener: Listener<TMessage>
  ) {
    this.lifecycle.onCleanup(this.reactor.on(topic, listener))
  }

  /**
   * Like {@link Reactor.once} but will automatically manage disposing the subscription when the current effect is disposed.
   *
   * @param topic - Reference to the topic, i.e. the message factory function.
   * @param listener - A function that will be called every time a message is emitted on the topic.
   */
  once<TMessage, TTrigger, TInitial>(
    topic: Topic<TMessage, TTrigger, TInitial>,
    listener: Listener<TMessage>
  ) {
    this.lifecycle.onCleanup(this.reactor.once(topic, listener))
  }

  /**
   * Create a JS interval that will be automatically cancelled when the current effect is disposed.
   */
  interval(callback: () => void, ms?: number | undefined) {
    const interval = setInterval(() => {
      callback()
    }, ms)
    this.lifecycle.onCleanup(() => clearInterval(interval))
  }

  /**
   * Create a JS timeout that will be automatically cancelled when the current effect is disposed.
   */
  timeout(callback: () => void, ms?: number | undefined): void {
    const interval = setInterval(() => {
      callback()
    }, ms)
    this.lifecycle.onCleanup(() => clearInterval(interval))
  }

  /**
   * Register a cleanup function for the current effect.
   *
   * @param cleanupHandler - A function that will be called when the current effect is disposed.
   */
  onCleanup(cleanupHandler: AsyncDisposer) {
    this.lifecycle.onCleanup(cleanupHandler)
  }

  /**
   * Create a subsidiary effect.
   *
   * @remarks
   *
   * This is the main way to compose your application. This will add a new effect to the reactive system and automatically manage its disposal when the current effect is disposed.
   *
   * The return value is the result of the first invocation of the effect.
   */
  use<TReturn>(effect: Effect<undefined, TReturn>): TReturn
  use<TProperties, TReturn>(
    effect: Effect<TProperties, TReturn>,
    properties: TProperties
  ): TReturn
  use<TProperties, TReturn>(
    effect: Effect<TProperties | undefined, TReturn>,
    properties?: TProperties
  ): TReturn {
    let isDisposed = false
    let waker: Waker
    let effectResult!: TReturn

    const run = async () => {
      for (;;) {
        if (isDisposed) return

        waker = new Waker()
        const lifecycle = new LifecycleScope()

        const context = new EffectContext(this.reactor, lifecycle, waker)

        effectResult = effect(context, properties)

        // Wait in case the effect is asynchronous.
        // eslint-disable-next-line @typescript-eslint/await-thenable
        await effectResult

        // Mark this waker as used which helps catch some cases of improper use of tracked methods like get()
        waker.dispose()

        await waker.promise

        await lifecycle.dispose()
      }
    }

    this.lifecycle.onCleanup(() => {
      isDisposed = true
      waker.resolve()
    })

    run().catch((error) => {
      logger.logError(error)
    })

    return effectResult
  }
}

export const useRootEffect = (
  reactor: Reactor,
  lifecycle: LifecycleScope,
  effect: Effect
) => {
  // Create a dummy effect context that can be "above" the root effect.
  const waker = new Waker()
  waker.dispose()
  const context = new EffectContext(reactor, lifecycle, waker)
  context.use(effect)
}
