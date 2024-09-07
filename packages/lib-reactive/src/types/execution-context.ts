import type { Promisable } from "type-fest"

import type { WrappedCallback } from "../internal/wrap-callback"

export interface ExecutionContext {
  callback: <TCallback extends (...parameters: unknown[]) => Promisable<void>>(
    callback: TCallback,
  ) => WrappedCallback<TCallback>
}
