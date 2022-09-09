import {
  createArrayEffect,
  createIndexedArrayEffect,
} from "./internal/array-effect"
import type { LifecycleScope } from "./internal/lifecycle-scope"
import { makePromise } from "./internal/promise"
import { AsyncDisposer, DisposeSymbol, Factory, Reactor } from "./reactor"
import type { SignalFactory } from "./signal"
import type { Listener, TopicFactory } from "./topic"

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

    readonly wake: () => void,

    private readonly effectName: string
  ) {}

  /**
   * Read the current value from a signal and automatically re-run the effect if the value changes.
   *
   * @remarks
   *
   * Tracking is only possible in the synchronous portion of the effect. If get is called after the synchronous execution, e.g. in response to an event or after an await, it will throw an error.
   *
   * To read a value without tracking, use {@link read}.
   *
   * @param signalFactory - Reference to the signal's factory function.
   * @param selector - Used to select only part of the message from a given topic. This can be useful to avoid re-running the effect if only an irrelevant portion of the message has changed.
   * @param comparator - By default, the reactor checks for strict equality (`===`) to determine if the value has changed. This can be overridden by passing a custom comparator function.
   * @returns The current value of the signal, narrowed by the selector.
   */
  get<TState>(signalFactory: SignalFactory<TState, never>): TState
  get<TState, TSelection>(
    signalFactory: SignalFactory<TState, never>,
    selector: (state: TState) => TSelection,
    comparator?: (oldValue: TSelection, newValue: TSelection) => boolean
  ): TSelection
  get<TState, TSelection>(
    signalFactory: SignalFactory<TState, never>,
    selector: (state: TState) => TSelection = (a) =>
      // Based on the overloaded function signature, the selector parameter may be omitted iff TMessage equals TSelection.
      // Therefore this cast is safe.
      a as unknown as TSelection,
    comparator: (a: TSelection, b: TSelection) => boolean = (a, b) => a === b
  ) {
    const notify = this.wake

    const handleTopicMessage = (message: TState) => {
      const newValue = selector(message)
      if (!comparator(value, newValue)) {
        // Once we have detected a change we can stop listening because once the effect re-runs it will create a new listener anyways.
        dispose()
        notify()
      }
    }

    const signal = this.use(signalFactory)
    const value = selector(signal.read())
    const dispose = signal.on(handleTopicMessage)

    this.lifecycle.onCleanup(dispose)

    return value
  }

  /**
   * Convenience method for extracting specific keys from a signal.
   *
   * @remarks
   *
   * This will automatically create the correct selector and comparator for the given keys. The effect will be re-run if any of the values for any of the keys change by strict equality.
   *
   * @param signal - Reference to the signal that should be queried.
   * @param keys - Tuple of keys that should be extracted from the signal.
   * @returns A filtered version of the signal state containing only the requested keys.
   */
  getKeys<TState, TKeys extends keyof TState>(
    signal: SignalFactory<TState, never>,
    keys: readonly TKeys[]
  ): Pick<TState, TKeys> {
    return this.get(
      signal,
      (state) => {
        const result = {} as Pick<TState, TKeys>
        for (const key of keys) {
          result[key] = state[key]
        }
        return result
      },
      (a, b) => {
        for (const key of keys) {
          if (a[key] !== b[key]) {
            return false
          }
        }
        return true
      }
    )
  }

  /**
   * Re-run this effect if a message is emitted on the given topic.
   *
   * @remarks
   *
   * Note that the effect will not necessarily re-run once per event. If multiple events are emitted on the topic before the effect is re-run, it will only re-run once.
   *
   * @param topic - Reference to the topic's factory function.
   */
  subscribe<TMessage>(topic: TopicFactory<TMessage>) {
    this.once(topic, this.wake)
  }

  /**
   * Like {@link TopicFactory.on} but will automatically manage disposing the subscription when the current effect is disposed.
   *
   * @param topic - Reference to the topic, i.e. the message factory function.
   * @param listener - A function that will be called every time a message is emitted on the topic.
   */
  on<TMessage>(topic: TopicFactory<TMessage>, listener: Listener<TMessage>) {
    this.lifecycle.onCleanup(
      this.use(topic).on((message) => {
        try {
          listener(message)
        } catch (error: unknown) {
          console.error("error in listener", {
            topic: topic.name,
            effect: this.effectName,
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
      this.use(topic).once((message) => {
        try {
          listener(message)
        } catch (error: unknown) {
          console.error("error in once listener", {
            topic: topic.name,
            effect: this.effectName,
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
      this.use(topic).on((message) => {
        listener(message).catch((error: unknown) => {
          console.error("error in async listener", {
            topic: topic.name,
            effect: this.effectName,
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
      this.use(topic).once((message) => {
        listener(message).catch((error: unknown) => {
          console.error("error in onceAsync listener", {
            topic: topic.name,
            effect: this.effectName,
            error,
          })
        })
      })
    )
  }

  /**
   * Create a JS interval that will be automatically cancelled when the current effect is disposed.
   */
  interval(callback: () => void, intervalInMilliseconds?: number | undefined) {
    const interval = setInterval(() => {
      try {
        callback()
      } catch (error) {
        console.error("error in interval callback", {
          effect: this.effectName,
          error,
        })
      }
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
      try {
        callback()
      } catch (error) {
        console.error("error in timeout callback", {
          effect: this.effectName,
          error,
        })
      }
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
   * Get a reference to a value in the context.
   *
   * If the value is not found, it will be instantiated.
   *
   * When the current effect lifecycle is disposed, the reference will be disposed as well.
   *
   * @param factory - Factory function identifying the context value that should be fetched.
   * @returns - The requested context value.
   */
  use<T>(factory: Factory<T>): T {
    const value = this.reactor.useContext(factory)

    const valueAsObject = value as Record<keyof never, unknown>
    if (
      DisposeSymbol in value &&
      typeof valueAsObject[DisposeSymbol] === "function"
    ) {
      this.lifecycle.onCleanup(valueAsObject[DisposeSymbol] as AsyncDisposer)
    }

    return value
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
  run<TReturn>(effect: Effect<undefined, TReturn>): TReturn
  run<TProperties, TReturn>(
    effect: Effect<TProperties, TReturn>,
    properties: TProperties
  ): TReturn
  run<TProperties, TReturn>(
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
      console.error("error in child effect", {
        effect: effect.name,
        parentEffect: this.effectName,
        error,
      })
    })

    return result
  }

  /**
   * Run an effect for each element of an array topic. When the array value changes, only the effects corresponding to changed elements will be re-run.
   */
  for<TElement, TReturn>(
    arrayTopicFactory: SignalFactory<readonly TElement[], never>,
    effect: Effect<TElement, TReturn>
  ) {
    return this.run(
      createArrayEffect(arrayTopicFactory, effect, this.effectName)
    )
  }

  /**
   * Like {@link for} but compares elements based on their index in the array. Also passes the index to the effect.
   */
  forIndex<TElement, TReturn>(
    arrayTopicFactory: SignalFactory<readonly TElement[], never>,
    effect: Effect<readonly [element: TElement, index: number], TReturn>
  ) {
    return this.run(
      createIndexedArrayEffect(arrayTopicFactory, effect, this.effectName)
    )
  }
}

export const runEffect = async <TProperties, TReturn>(
  reactor: Reactor,
  effect: Effect<TProperties, TReturn>,
  properties: TProperties,
  parentLifecycle: LifecycleScope,
  resultCallback?: (result: TReturn) => void
) => {
  for (;;) {
    if (parentLifecycle.isDisposed) return

    const lifecycle = parentLifecycle.deriveChildLifecycle()
    const waker = makePromise()

    const context = new EffectContext(
      reactor,
      lifecycle,
      waker.resolve,
      effect.name
    )

    try {
      const effectResult = effect(context, properties)

      // runEffect MUST always call the resultCallback or throw an error
      if (resultCallback) resultCallback(effectResult)

      // --- There must be no `await` before calling the resultCallback ---

      // Wait in case the effect is asynchronous.
      // eslint-disable-next-line @typescript-eslint/await-thenable
      await effectResult

      await waker
    } finally {
      await lifecycle.dispose()
    }
  }
}
