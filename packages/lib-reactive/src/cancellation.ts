import { isObject } from "@dassie/lib-type-utils"

import { createDeferred } from "./deferred"

export const CancellationSymbol = Symbol("das:reactive:cancellation")

/**
 * An object that describes a cancellation that has occurred.
 */
interface Cancellation {
  [CancellationSymbol]: true

  /**
   * A string which describes the reason for the cancellation.
   */
  readonly reason: string
}

/**
 * A read-only view of a Cancellable.
 */
interface ReadonlyCancellable extends PromiseLike<Cancellation> {
  /**
   * Whether the cancellation has already occurred.
   */
  readonly isCancelled: boolean

  /**
   * The cancellation object if the cancellation has already occurred.
   */
  readonly cancellation: Cancellation | undefined

  /**
   * An AbortSignal which will be aborted when the cancellation occurs.
   */
  readonly abortSignal: AbortSignal
}

export interface Cancellable extends ReadonlyCancellable {
  cancel(reason: string): void
}

export class CancellableImplementation implements PromiseLike<Cancellation> {
  #deferred = createDeferred<Cancellation>()
  cancellation: Cancellation | undefined
  #abortController: AbortController | undefined

  // eslint-disable-next-line unicorn/no-thenable
  then = this.#deferred.then.bind(this.#deferred)

  get reason() {
    return this.cancellation?.reason
  }

  get isCancelled() {
    return this.cancellation !== undefined
  }

  get abortSignal() {
    if (!this.#abortController) {
      this.#abortController = new AbortController()
    }
    return this.#abortController.signal
  }

  cancel(reason: string) {
    if (this.isCancelled) return

    this.cancellation = {
      [CancellationSymbol]: true,
      reason,
    }

    this.#deferred.resolve(this.cancellation)

    if (this.#abortController) {
      this.#abortController.abort()
    }
  }
}

export function createCancellable(): Cancellable {
  return new CancellableImplementation()
}

export const isCancellation = (object: unknown): object is Cancellable =>
  isObject(object) && object[CancellationSymbol] === true
