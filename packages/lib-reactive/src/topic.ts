import type { Promisable } from "type-fest"

import { createLogger } from "@xen-ilp/lib-logger"
import { isObject } from "@xen-ilp/lib-type-utils"

import type { Disposer } from "./reactor"

export type Listener<TMessage> = (message: TMessage) => Promisable<void>

export const TopicSymbol = Symbol("xen:reactive:topic")

export type TopicFactory<TMessage = unknown, TTrigger = never> = () => Topic<
  TMessage,
  TTrigger
>

export interface Topic<TMessage = never, TTrigger = TMessage> {
  /**
   * Marks this object as a topic.
   */
  [TopicSymbol]: true

  /**
   * Subscribe to a topic.
   *
   * @remarks
   *
   * Allows you to listen to events happening inside of the reactor. Every time a message is emitted on a topic, the listener will be called.
   *
   * Returns a function that when called will unsubscribe from the topic.
   *
   * @param listener - A function that will be called every time a message is emitted on the topic.
   * @returns A function which if called will unsubscribe the listener from the topic.
   */
  on: (listener: Listener<TMessage>) => Disposer

  /**
   * Subscribe to receive a single message from a topic. Similar to {@link on} but will only call the listener once.
   *
   * @param listener - A function that will be called once the next time a message is emitted on the topic.
   * @returns A function which if called will unsubscribe the listener from the topic.
   */
  once: (listener: Listener<TMessage>) => Disposer

  /**
   * Emit a message to all listeners of a topic.
   *
   * @remarks
   *
   * You should use this method when you don't care about any error that might occur during processing.
   *
   * If you want to catch errors, use {@link emitAndWait} instead.
   *
   * @param trigger - Value to pass to the message factory function.
   */
  emit: (trigger: TTrigger) => void

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
   * @param parameters - Any parameters to pass to the message factory function.
   */
  emitAndWait: (input: TTrigger) => Promise<void>
}

export const createTopic = <TMessage>(): Topic<TMessage, TMessage> => {
  const logger = createLogger(`xen:reactive:topic`)

  // We construct a temporary object in order to assign the name to the function
  // eslint-disable-next-line @typescript-eslint/no-non-null-assertion
  let listeners: Listener<TMessage> | Set<Listener<TMessage>> | undefined

  const emit = (message: TMessage) => {
    if (typeof listeners === "function") {
      try {
        void listeners(message)
      } catch (error) {
        logger.error("error in listener", { error })
      }
      return
    } else if (listeners == undefined) {
      return
    }

    for (const listener of listeners) {
      try {
        void listener(message)
      } catch (error) {
        logger.error("error in listener", { error })
      }
    }
  }

  const emitAndWait = async (message: TMessage): Promise<void> => {
    if (typeof listeners === "function") {
      try {
        await listeners(message)
      } catch (error) {
        logger.error("error in listener", { error })
      }
      return
    } else if (listeners == undefined) {
      return
    }

    // We use a for loop instead of Array.map to create a shorter and more readable call stack
    const promises = Array.from<PromiseLike<void>>({
      length: listeners.size,
    })
    for (const [index, listener] of [...listeners].entries()) {
      promises[index] = Promise.resolve(listener(message))
    }
    await Promise.all(promises)
  }

  const on = (listener: Listener<TMessage>) => {
    if (typeof listeners === "function") {
      listeners = new Set([listeners, listener])
    } else if (listeners == undefined) {
      listeners = listener
    } else {
      listeners.add(listener)
    }

    return () => {
      if (typeof listeners === "function") {
        if (listeners === listener) {
          listeners = undefined
        }
      } else if (listeners != undefined) {
        listeners.delete(listener)

        if (listeners.size === 1) {
          // Generally it's better to use an iterator to get the first element from a set because it's more performant for large sets. But in this case the size of the set is always one so it's actually faster to use the array syntax.
          // See: https://tinyurl.com/perf-set-get-sole-element
          listeners = [...listeners.values()][0]
        }
      }
    }
  }

  const once = (listener: Listener<TMessage>) => {
    const disposer = on(async (message) => {
      disposer()
      return await listener(message)
    })

    return disposer
  }

  return { [TopicSymbol]: true, on, once, emit, emitAndWait }
}

export const isTopic = (object: unknown): object is Topic =>
  isObject(object) && object[TopicSymbol] === true
