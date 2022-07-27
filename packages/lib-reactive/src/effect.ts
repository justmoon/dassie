import { createLogger } from "@xen-ilp/lib-logger"

import { LifecycleScope } from "./internal/lifecycle-scope"
import { Waker } from "./internal/waker"
import type { AsyncDisposer, Reactor } from "./reactor"
import type { StoreFactory } from "./store"
import type { Listener, TopicFactory } from "./topic"

const logger = createLogger("xen:reactive")

export type Effect<TProperties = unknown, TReturn = unknown> = (
  sig: EffectContext,
  properties: TProperties
) => TReturn

export type AsyncListener<TMessage> = (message: TMessage) => Promise<void>

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

    private readonly waker: Waker,

    private readonly effect: Effect<never>
  ) {}

  /**
   * Read the current value from a store and automatically re-run the effect if the value changes.
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
  get<TState>(topic: StoreFactory<TState>): TState
  get<TState, TSelection>(
    topic: StoreFactory<TState>,
    selector: (state: TState) => TSelection,
    comparator?: (oldValue: TSelection, newValue: TSelection) => boolean
  ): TSelection
  get<TState, TSelection>(
    topic: StoreFactory<TState>,
    selector: (state: TState) => TSelection = (a) =>
      // Based on the overloaded function signature, the selector parameter may be omitted iff TMessage equals TSelection.
      // Therefore this cast is safe.
      a as unknown as TSelection,
    comparator: (a: TSelection, b: TSelection) => boolean = (a, b) => a === b
  ) {
    const handleTopicMessage = (message: TState) => {
      const newValue = selector(message)
      if (!comparator(value, newValue)) {
        // Once we have detected a change we can stop listening because once the effect re-runs it will create a new listener anyways.
        dispose()
        notify()
      }
    }

    const notify = this.waker.notify
    const value = selector(this.reactor.useContext(topic).read())
    const dispose = this.reactor.useContext(topic).on(handleTopicMessage)
    this.lifecycle.onCleanup(dispose)
    return value
  }

  /**
   * Like {@link get}, but without tracking. That means that calling read will not cause the effect to be re-run if the value of the given store changes.
   *
   * @param topic - Reference to the topic, i.e. the message factory function.
   */
  read<TState>(store: StoreFactory<TState>): TState {
    return this.reactor.useContext(store).read()
  }

  /**
   * Re-run this effect if a message is emitted on the given topic.
   *
   * @param topic - Reference to the topic, i.e. its factory function.
   */
  subscribe<TMessage>(topic: TopicFactory<TMessage>) {
    this.once(topic, this.waker.notify)
  }

  /**
   * Like {@link TopicFactory.on} but will automatically manage disposing the subscription when the current effect is disposed.
   *
   * @param topic - Reference to the topic, i.e. the message factory function.
   * @param listener - A function that will be called every time a message is emitted on the topic.
   */
  on<TMessage>(topic: TopicFactory<TMessage>, listener: Listener<TMessage>) {
    this.lifecycle.onCleanup(
      this.reactor.useContext(topic).on((message) => {
        try {
          listener(message)
        } catch (error: unknown) {
          logger.error("error in listener", {
            topic: topic.name,
            effect: this.effect.name,
            error,
          })
        }
      })
    )
  }

  /**
   * Like {@link TopicFactory.once} but will automatically manage disposing the subscription when the current effect is disposed.
   *
   * @param topic - Reference to the topic, i.e. the message factory function.
   * @param listener - A function that will be called every time a message is emitted on the topic.
   */
  once<TMessage>(topic: TopicFactory<TMessage>, listener: Listener<TMessage>) {
    this.lifecycle.onCleanup(
      this.reactor.useContext(topic).once((message) => {
        try {
          listener(message)
        } catch (error: unknown) {
          logger.error("error in once listener", {
            topic: topic.name,
            effect: this.effect.name,
            error,
          })
        }
      })
    )
  }

  /**
   * Like {@link on} but handles errors in async listeners.
   *
   * @param topic - Topic that the message should be sent to.
   * @param listener - An async function that will be called every time a message is emitted on the topic.
   */
  onAsync<TMessage>(
    topic: TopicFactory<TMessage>,
    listener: AsyncListener<TMessage>
  ) {
    this.lifecycle.onCleanup(
      this.reactor.useContext(topic).on((message) => {
        listener(message).catch((error: unknown) => {
          logger.error("error in async listener", {
            topic: topic.name,
            effect: this.effect.name,
            error,
          })
        })
      })
    )
  }

  /**
   * Like {@link once} but handles errors in async listeners.
   *
   * @param topic - Topic that the message should be sent to.
   * @param listener - An async function that will be called every time a message is emitted on the topic.
   */
  onceAsync<TMessage>(
    topic: TopicFactory<TMessage>,
    listener: AsyncListener<TMessage>
  ) {
    this.lifecycle.onCleanup(
      this.reactor.useContext(topic).once((message) => {
        listener(message).catch((error: unknown) => {
          logger.error("error in onceAsync listener", {
            topic: topic.name,
            effect: this.effect.name,
            error,
          })
        })
      })
    )
  }

  /**
   * This is a shorthand for {@link TopicFactory.emit} to quickly send a message to a topic.
   *
   * @param topic - Topic that the message should be sent to.
   * @param trigger - Message to send to the topic.
   */
  emit<TTrigger>(topic: TopicFactory<unknown, TTrigger>, trigger: TTrigger) {
    this.reactor.useContext(topic).emit(trigger)
  }

  /**
   * Create a JS interval that will be automatically cancelled when the current effect is disposed.
   */
  interval(callback: () => void, intervalInMilliseconds?: number | undefined) {
    const interval = setInterval(() => {
      callback()
    }, intervalInMilliseconds)
    this.lifecycle.onCleanup(() => clearInterval(interval))
  }

  /**
   * Create a JS timeout that will be automatically cancelled when the current effect is disposed.
   */
  timeout(
    callback: () => void,
    delayInMilliseconds?: number | undefined
  ): void {
    const timer = setTimeout(() => {
      callback()
    }, delayInMilliseconds)
    this.lifecycle.onCleanup(() => clearTimeout(timer))
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
   * Create a child effect.
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
    let result!: TReturn

    runEffect(
      this.reactor,
      effect,
      properties,
      this.lifecycle,
      (_result) => (result = _result)
    ).catch((error: unknown) => {
      logger.error("error in child effect", {
        effect: effect.name,
        parentEffect: this.effect.name,
        error,
      })
    })

    return result
  }
}

export const runEffect = async <TProperties, TReturn>(
  reactor: Reactor,
  effect: Effect<TProperties, TReturn>,
  properties: TProperties,
  parentLifecycle: LifecycleScope,
  resultCallback?: (result: TReturn) => void
) => {
  let waker = new Waker()
  let lifecycle = new LifecycleScope()
  let effectResult: TReturn

  parentLifecycle.onCleanup(async () => {
    await lifecycle.dispose()
    waker.resolve()
  })

  for (;;) {
    if (lifecycle.isDisposed || parentLifecycle.isDisposed) return

    const context = new EffectContext(reactor, lifecycle, waker, effect)

    effectResult = effect(context, properties)

    // runEffect MUST always call the resultCallback or throw an error
    if (resultCallback) resultCallback(effectResult)

    // --- There must be no `await` before calling the resultCallback ---

    // Wait in case the effect is asynchronous.
    // eslint-disable-next-line @typescript-eslint/await-thenable
    await effectResult

    // Mark this waker as used which helps catch some cases of improper use of tracked methods like get()
    waker.dispose()

    await waker.promise

    await lifecycle.dispose()

    waker = new Waker()
    lifecycle = new LifecycleScope()
  }
}
