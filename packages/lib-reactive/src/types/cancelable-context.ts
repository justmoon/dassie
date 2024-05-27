import type { Cancellable } from "../cancellation"

export interface CancelableContext {
  readonly cancellable: Cancellable
  readonly abortSignal: AbortSignal
}
