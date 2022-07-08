import type { AsyncOrSync } from "ts-essentials"

import { createLogger } from "@xen-ilp/lib-logger"

import type { Topic } from "./create-topic"
import { LifecycleScope } from "./internal/lifecycle-scope"
import { EffectContext, useRootEffect } from "./use-effect"

const logger = createLogger("xen:reactive")

export type Effect<TProperties = unknown, TReturn = unknown> = (
  sig: EffectContext,
  properties: TProperties
) => TReturn
export type Listener<TMessage> = (message: TMessage) => AsyncOrSync<void>
export type Disposer = () => void
export type AsyncDisposer = () => AsyncOrSync<void>

export interface Reactor {
  /**
   * Subscribe to a topic.
   *
   * @remarks
   *
   * Allows you to listen to events happening inside of the reactor. Every time a message is emitted on a topic, the listener will be called.
   *
   * Returns a function that when called will unsubscribe from the topic.
   *
   * @param topic - A reference to the topic to subscribe to. Note that stores are also just topics.
   * @param listener - A function that will be called every time a message is emitted on the topic.
   * @returns A function which if called will unsubscribe the listener from the topic.
   */
  on: <TMessage, TTrigger, TState>(
    topic: Topic<TMessage, TTrigger, TState>,
    listener: Listener<TMessage>
  ) => Disposer

  /**
   * Subscribe to receive a single message from a topic. Similar to {@link on} but will only call the listener once.
   *
   * @param topic - A reference to the topic to subscribe to. Note that stores are also just topics.
   * @param listener - A function that will be called once the next time a message is emitted on the topic.
   * @returns A function which if called will unsubscribe the listener from the topic.
   */
  once: <TMessage, TTrigger, TState>(
    topic: Topic<TMessage, TTrigger, TState>,
    listener: Listener<TMessage>
  ) => Disposer

  /**
   * Emit a message to all listeners of a topic.
   *
   * @remarks
   *
   * You should use this method when you don't care about any error that might occur during processing.
   *
   * If you want to catch errors, use {@link emitAndWait} instead.
   *
   * @param topic - Reference to the topic, i.e. the message factory function.
   * @param input - Value to pass to the message factory function.
   */
  emit: <TMessage, TTrigger, TInitial>(
    topic: Topic<TMessage, TTrigger, TInitial>,
    message: TTrigger
  ) => void

  /**
   * Emit a message to all listeners of a topic and either resolve after all listeners complete or reject with the first error encountered.
   *
   * @remarks
   *
   * You should use this method when you want the caller to handle any error that might occur during processing.
   *
   * While this method has some benefits, there are also some significant drawbacks. Normally, callers should not care what happens to the messages they send.
   *
   * In general, you should use this for short, tightly coupled processing pipelines and use {@link emit} for everything else. For example, we use this to keep track of any errors that happen during the processing of a single HTTP request. While processing a request, we may emit messages that kick off other, longer-running chains of tasks, but for that we would use {@link emit} instead.
   *
   * @param topic - Reference to the topic's message factory function.
   * @param parameters - Any parameters to pass to the message factory function.
   */
  emitAndWait: <TMessage, TTrigger, TInitial>(
    topic: Topic<TMessage, TTrigger, TInitial>,
    input: TTrigger
  ) => Promise<void>

  /**
   * Read the most recent value from a topic.
   *
   * @remarks
   *
   * This method is similar to {@link get} but will not track updates.
   *
   * @param topic - Reference to the topic, i.e. the message factory function.
   * @returns The most recent value from the topic or the initial value if there have not been any messages on this topic yet.
   */
  read: <TMessage, TTrigger, TInitial>(
    topic: Topic<TMessage, TTrigger, TInitial>
  ) => TMessage | TInitial

  /**
   * Returns the next message from a topic as a promise.
   *
   * @remarks
   *
   * This method is similar to once but may be more convenient to use inside of an async function. It will return a promise that will resolve with the next message from the topic.
   *
   * @param topic - Reference to the topic, i.e. the message factory function.
   * @returns A promise that will resolve with the next message from the topic.
   */
  readNext: <TMessage, TTrigger, TInitial>(
    topic: Topic<TMessage, TTrigger, TInitial>
  ) => Promise<TMessage>

  /**
   * Register a cleanup handler for this reactor.
   *
   * @remarks
   *
   * The handler will be called when the reactor is disposed.
   *
   * @param cleanupHandler - A function that will be called when the reactor is disposed.
   */
  onCleanup: (cleanupHandler: () => void) => void

  /**
   * Dispose of the entire reactive system.
   */
  dispose: AsyncDisposer
}

interface TopicState<TMessage = unknown, TInitial = unknown> {
  /**
   * The most recently emitted value.
   *
   * @remarks
   *
   * For stores, this will be the current state of the store.
   */
  value: TMessage | TInitial
  listeners: Set<Listener<TMessage>>
}

interface TopicsCache extends Map<Topic, TopicState> {
  get<TMessage, TInitial>(
    key: Topic<TMessage, never, TInitial>
  ): TopicState<TMessage, TInitial> | undefined
  set<TMessage, TInitial>(
    key: Topic<TMessage, never, TInitial>,
    value: TopicState<TMessage, TInitial>
  ): this
}

export const createReactor = (rootEffect: Effect): Reactor => {
  const topics: TopicsCache = new Map()

  const lifecycle = new LifecycleScope()

  const read = <TMessage, TInitial>(
    topic: Topic<TMessage, never, TInitial>
  ): TMessage | TInitial => {
    const state = topics.get(topic)
    return state ? state.value : topic.initialValue
  }

  const readNext = async <TMessage, TInitial>(
    topic: Topic<TMessage, never, TInitial>
  ): Promise<TMessage> => {
    return await new Promise((resolve) => once(topic, resolve))
  }

  const getOrCreateState = <TMessage, TState>(
    topic: Topic<TMessage, never, TState>
  ): TopicState<TMessage, TState> => {
    let topicState = topics.get(topic)

    if (!topicState) {
      topicState = {
        value: topic.initialValue,
        listeners: new Set(),
      }

      topics.set(topic, topicState)
    }
    return topicState
  }

  const emit = <TMessage, TTrigger, TInitial>(
    topic: Topic<TMessage, TTrigger, TInitial>,
    input: TTrigger
  ) => {
    const state = getOrCreateState(topic)
    const message = topic(input, state.value)
    state.value = message

    if (state.listeners.size === 0) {
      return
    }

    for (const listener of state.listeners) {
      Promise.resolve(listener(message)).catch((error) => {
        logger.error("error in listener", {
          topic: topic.name || "anonymous",
        })
        logger.logError(error)
      })
    }
  }

  const emitAndWait = async <TMessage, TTrigger, TInitial>(
    topic: Topic<TMessage, TTrigger, TInitial>,
    input: TTrigger
  ): Promise<void> => {
    const state = getOrCreateState(topic)
    const message = topic(input, state.value)
    state.value = message

    if (state.listeners.size === 0) {
      return
    }

    // We use a for loop instead of Array.map to create a shorter and more readable call stack
    const promises = Array.from<PromiseLike<void>>({
      length: state.listeners.size,
    })
    for (const [index, listener] of [...state.listeners].entries()) {
      promises[index] = Promise.resolve(listener(message))
    }
    await Promise.all(promises)
  }

  const on = <TMessage, TTrigger, TState>(
    topic: Topic<TMessage, TTrigger, TState>,
    listener: Listener<TMessage>
  ) => {
    const state = getOrCreateState(topic)

    state.listeners.add(listener)

    return () => {
      state.listeners.delete(listener)
    }
  }

  const once = <TMessage, TTrigger, TState>(
    topic: Topic<TMessage, TTrigger, TState>,
    listener: Listener<TMessage>
  ) => {
    const disposer = on(topic, async (message) => {
      disposer()
      return await listener(message)
    })

    return disposer
  }

  const reactor: Reactor = {
    on,
    once,
    emit,
    emitAndWait,
    read,
    readNext,
    onCleanup: lifecycle.onCleanup.bind(lifecycle),
    dispose: lifecycle.dispose.bind(lifecycle),
  }

  useRootEffect(reactor, lifecycle, rootEffect)

  return reactor
}
