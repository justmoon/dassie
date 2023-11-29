import { Promisable } from "type-fest"

import { WrappedCallback } from "../internal/wrap-callback"

export interface ExecutionContext {
  callback: <TCallback extends (...parameters: unknown[]) => Promisable<void>>(
    callback: TCallback,
  ) => WrappedCallback<TCallback>
}
