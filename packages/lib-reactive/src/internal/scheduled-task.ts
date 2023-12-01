import { Promisable } from "type-fest"

import { UnreachableCaseError, isThenable } from "@dassie/lib-type-utils"

import { Time, TimeoutId } from "../types/base-modules/time"
import { LifecycleContext } from "../types/lifecycle-context"

/**
 * Description of a task that should be executed periodically.
 *
 * @remarks
 *
 * Some parameter names and values are copied from the excellent [Temporal.io API](https://docs.temporal.io/) under fair
 * use. If you need a cloud-based scheduling engine, go check them out!
 */
export interface TaskDescriptor {
  /**
   * Task to be executed.
   */
  handler: (parameters: TaskParameters) => Promisable<void>

  /**
   * Time between executions of the task, in milliseconds.
   */
  interval: number

  /**
   * Maximum additional time to be randomly added between executions of the task, in milliseconds.
   *
   * @remarks
   *
   * Defaults to `0`.
   */
  jitter?: number

  /**
   * Policy for handling overlapping executions of the task.
   *
   * * `skip` (default) - Skip the execution if the previous execution is still running.
   * * `bufferOne` - Buffer the execution if the previous execution is still running. If multiple executions are
   *   buffered, only one will be executed.
   * * `bufferAll` - Buffer the execution if the previous execution is still running. If multiple executions are
   *   buffered, all of them will be executed sequentially.
   * * `cancelOther` - Cancel the previous execution if it is still running.
   * * `allowAll` - Allow all executions to run concurrently.
   */
  overlapPolicy?:
    | "skip"
    | "bufferOne"
    | "bufferAll"
    | "cancelOther"
    | "allowAll"
}

export interface TaskParameters {
  abortSignal: AbortSignal
}

export interface ExecutionInstance {
  returnValue: PromiseLike<void>
  abortController: AbortController
}

export interface ScheduledTask {
  /**
   * Execute the task immediately.
   */
  execute: () => Promise<void>

  /**
   * Schedule an execution of the task after the specified interval.
   */
  schedule: () => void

  /**
   * Cancel any scheduled executions of the task.
   */
  cancel: () => void

  /**
   * Cancel any scheduled executions of this task and abort any currently running executions.
   */
  cancelAndAbort: (this: void) => void
}

class ScheduledTaskImplementation implements ScheduledTask {
  private readonly instances: Set<ExecutionInstance> = new Set()

  /**
   * Number of executions that are buffered behind the current one.
   */
  private buffer: number = 0

  private timer: TimeoutId | undefined

  constructor(
    private readonly context: LifecycleContext,
    private readonly descriptor: TaskDescriptor,
    private readonly time: Time,
  ) {}

  async execute() {
    try {
      const abortController = new AbortController()
      const returnValue = this.descriptor.handler({
        abortSignal: abortController.signal,
      })

      // If the handler was synchronous, we can skip the asynchronous tracking logic
      if (!isThenable(returnValue)) {
        return
      }

      const executionInstance = {
        returnValue,
        abortController,
      }

      this.instances.add(executionInstance)

      await returnValue

      this.instances.delete(executionInstance)

      if (!this.timer && this.instances.size === 0) {
        this.context.lifecycle.offCleanup(this.cancelAndAbort)
      }
    } catch (error: unknown) {
      console.error("error in scheduled task", { error })
    }
  }

  /**
   * Schedule the next execution of the task.
   *
   * @remarks
   *
   * If another execution is already scheduled, it will be cancelled and rescheduled. If another execution is already
   * running, this will not affect this method, i.e. another execution will be scheduled after the given interval.
   */
  schedule() {
    if (this.timer) {
      this.time.clearTimeout(this.timer)
    }

    const { interval, jitter } = this.descriptor
    const delay = interval + Math.random() * (jitter ?? 0)
    this.timer = this.time.setTimeout(() => void this.tick(), delay)

    this.context.lifecycle.onCleanup(this.cancelAndAbort)
  }

  cancel() {
    if (this.timer) {
      this.time.clearTimeout(this.timer)
      this.timer = undefined

      if (this.instances.size === 0) {
        this.context.lifecycle.offCleanup(this.cancelAndAbort)
      }
    }
  }

  cancelAndAbort = () => {
    this.cancel()
    for (const instance of this.instances) {
      instance.abortController.abort()
    }
  }

  private async tick() {
    this.timer = undefined
    this.schedule()

    if (this.instances.size > 0) {
      const overlapPolicy = this.descriptor.overlapPolicy ?? "skip"
      switch (overlapPolicy) {
        case "skip": {
          return
        }
        case "bufferOne": {
          if (this.buffer > 0) return
          this.buffer = 1
          return
        }
        case "bufferAll": {
          this.buffer++
          return
        }
        case "cancelOther": {
          for (const instance of this.instances) {
            instance.abortController.abort()
          }
          break
        }
        case "allowAll": {
          break
        }
        default: {
          throw new UnreachableCaseError(overlapPolicy)
        }
      }
    }

    await this.execute()

    while (this.buffer > 0) {
      this.buffer--
      await this.execute()
    }
  }
}

export const createScheduledTask = (
  context: LifecycleContext,
  descriptor: TaskDescriptor,
  time: Time,
): ScheduledTask => new ScheduledTaskImplementation(context, descriptor, time)
