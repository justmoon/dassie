import { Promisable } from "type-fest"

import { isObject } from "@dassie/lib-type-utils"

import { LifecycleScope } from "../lifecycle"
import { StatefulContext } from "../types/stateful-context"

export interface WrappedCallback<
  TCallback extends (...parameters: unknown[]) => Promisable<unknown>,
> {
  (...parameters: Parameters<TCallback>): void
  original: TCallback
}

export const wrapCallback = <
  TCallback extends (...parameters: unknown[]) => Promisable<unknown>,
>(
  callback: TCallback,
  lifecycle: LifecycleScope & StatefulContext<object>,
  path?: string | undefined,
): WrappedCallback<TCallback> => {
  const wrappedCallback: WrappedCallback<TCallback> = Object.assign(
    ((...parameters) => {
      if (lifecycle.isDisposed) {
        lifecycle.reactor.debug?.warnAboutDanglingCallback(
          callback.name,
          lifecycle,
          path,
        )

        return
      }

      try {
        const result = callback(...parameters)

        if (
          isObject(result) &&
          "then" in result &&
          typeof result["then"] === "function"
        ) {
          result["then"](undefined, (error: unknown) => {
            console.error("error in async callback", {
              name: callback.name ?? "anonymous",
              path,
              error,
            })
          })
        }
      } catch (error) {
        console.error("error in callback", {
          name: callback.name ?? "anonymous",
          path,
          error,
        })
      }
    }) as TCallback,
    {
      original: callback,
    },
  )

  return wrappedCallback
}
