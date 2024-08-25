/*
 * Based on code from the TRPC project. Used under MIT license. Original code is
 * Copyright (c) 2023 Alex Johansson.
 */

export interface ProxyCallbackOptions {
  path: string[]
  parameters: unknown[]
}
export type ProxyCallback = (options: ProxyCallbackOptions) => unknown

function RpcProxy() {
  // no-op
}

function createInnerProxy(callback: ProxyCallback, path: string[]) {
  const proxy: unknown = new Proxy(RpcProxy, {
    get(_, key) {
      if (typeof key !== "string" || key === "then") {
        // special case for if the proxy is accidentally treated
        // like a PromiseLike (like in `Promise.resolve(proxy)`)
        return undefined
      }
      if (key === "path") {
        return path
      }
      return createInnerProxy(callback, [...path, key])
    },
    apply(_1, _2, parameters: unknown[]) {
      const isApply = path.at(-1) === "apply"
      return callback({
        parameters:
          isApply ?
            ((parameters[1] as unknown[] | undefined) ?? [])
          : parameters,
        path: isApply ? path.slice(0, -1) : path,
      })
    },
  })

  return proxy
}

/**
 * Creates a proxy that calls the callback with the path and parameters
 */
export const createRecursiveProxy = (callback: ProxyCallback) =>
  createInnerProxy(callback, [])
