import type { AsyncOrSync } from "ts-essentials"

import { createLogger } from "@xen-ilp/lib-logger"

const logger = createLogger("xen:node:message-broker")

export type Topic<R = unknown, P extends never[] = never[]> = (
  ...parameters: P
) => R

export type Listener<T extends Topic> = (
  message: ReturnType<T>
) => AsyncOrSync<void>

export const createTopic = <T>(name: string) =>
  // We construct a temporary object which will serve to assign the name to the function
  // eslint-disable-next-line @typescript-eslint/no-non-null-assertion
  ({ [name]: (message: T) => message }[name]!)

export interface InstanceMap extends Map<Topic, Set<Listener<Topic>>> {
  get<T extends Topic>(key: T): Set<Listener<T>> | undefined
  set<T extends Topic>(key: T, value: Set<Listener<T>>): this
}

export default class MessageBroker {
  readonly listenersMap: InstanceMap = new Map()

  getListeners<T extends Topic>(topic: T) {
    let listeners = this.listenersMap.get(topic)
    if (!listeners) {
      listeners = new Set()
      this.listenersMap.set(topic, listeners)
    }
    return listeners
  }

  addListener<T extends Topic>(
    topic: T,
    listener: (message: ReturnType<T>) => AsyncOrSync<void>
  ) {
    const listeners = this.getListeners(topic)
    listeners.add(listener)

    return () => {
      listeners.delete(listener)
      if (listeners.size === 0) {
        this.listenersMap.delete(topic)
      }
    }
  }

  clearListeners<T extends Topic>(topic: T) {
    this.listenersMap.delete(topic)
  }

  /**
   * Emit a message to all listeners of a topic.
   *
   * @remarks
   *
   * You should use this method when you don't care about any error that might occur during processing.
   *
   * If you want to catch errors, use {@link emitAndWait} instead.
   *
   * @param topic - Reference to the topic's message factory function.
   * @param parameters - Any parameters to pass to the message factory function.
   */
  emit<T extends Topic<ReturnType<T>>>(topic: T, ...parameters: Parameters<T>) {
    const listeners = this.getListeners(topic)
    const message = topic(...parameters)

    if (listeners.size === 0) {
      logger.debug(`No listeners for ${topic.name}`)
    }

    for (const listener of listeners) {
      Promise.resolve(listener(message)).catch((error) => {
        logger.error("error in listener", { topic: topic.name || "anonymous" })
        logger.logError(error)
      })
    }
  }

  /**
   * Emit a message to all listeners of a topic and either wait until all processing completes or throw the first error encountered.
   *
   * @remarks
   *
   * You should use this method when you want to handle any error that might occur during processing.
   *
   * While this method has some benefits, there are also some significant drawbacks. Because this will keep the caller on the stack until all processing completes, it can create memory leaks and other issues when message chains get very long.
   *
   * In general, you should use this for short, tightly coupled processing pipelines and use {@link emit} for everything else. For example, we use this to keep track of any errors that happen during the processing of a single HTTP request. While processing a request, we may emit messages that kick off other, longer-running chains of tasks, but for that we would use {@link emit} instead.
   *
   * @param topic - Reference to the topic's message factory function.
   * @param parameters - Any parameters to pass to the message factory function.
   */
  async emitAndWait<T extends Topic<ReturnType<T>>>(
    topic: T,
    ...parameters: Parameters<T>
  ) {
    const listeners = this.getListeners(topic)
    const message = topic(...parameters)

    if (listeners.size === 0) {
      logger.debug(`No listeners for ${topic.name}`)
    }

    // We use a for loop instead of Array.map to create a shorter and more readable call stack
    const promises = Array.from<PromiseLike<void>>({ length: listeners.size })
    for (const [index, listener] of [...listeners].entries()) {
      promises[index] = Promise.resolve(listener(message))
    }
    await Promise.all(promises)
  }
}
