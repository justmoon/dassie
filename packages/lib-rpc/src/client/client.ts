import type { Promisable } from "type-fest"

import { UnreachableCaseError } from "@dassie/lib-type-utils"

import {
  type CancelEnvelope,
  type RequestEnvelope,
  type ResponseEnvelope,
  type ServerEnvelope,
  clientEnvelopeSchema,
} from "../common/envelope"
import {
  ROUTE_HANDLER_TO_TYPE_MAP,
  VALID_ROUTE_VERBS,
} from "../common/route-type"
import { type Disposer, createSubscription } from "../common/subscription"
import type { Transformer } from "../common/types/transformer"
import type { AnyRouter } from "../server/router/router"
import { createIdGenerator } from "./id-generator"
import { type ProxyCallbackOptions, createRecursiveProxy } from "./proxy"
import type { DeriveClientRouter } from "./types/client-route"

export interface ClientOptions {
  transformer?: Transformer | undefined
  connection: OutboundConnection
}

export type AnyClient = Client<AnyRouter>

export interface SubscriptionOptions<TData, TError> {
  onData: (data: TData) => void
  onError?: (error: TError) => void
}

export interface Client<TRouter extends AnyRouter> {
  rpc: DeriveClientRouter<TRouter>

  query: (path: string[], input: unknown) => Promise<unknown>
  mutate: (path: string[], input: unknown) => Promise<unknown>
  subscribe: (
    path: string[],
    input: unknown,
    options: SubscriptionOptions<unknown, unknown>,
  ) => Disposer

  close: () => void
}

export interface OutboundConnection {
  received: AsyncIterable<string>
  send: (data: string) => Promisable<void>
  resets?: AsyncIterable<void>
  close: () => void
}

interface SubscriptionEntry {
  request: RequestEnvelope
  onData: (data: unknown) => void
}

export function createClient<TRouter extends AnyRouter>({
  transformer = JSON,
  connection: { received, send, resets, close },
}: ClientOptions): Client<TRouter> {
  const generateId = createIdGenerator()
  const callbacks = new Map<string, (envelope: ResponseEnvelope) => void>()
  const subscriptions = new Map<string, SubscriptionEntry>()

  async function sendEnvelope(envelope: ServerEnvelope) {
    await send(transformer.stringify(envelope))
  }

  async function call(
    path: string[],
    type: "query" | "mutation",
    input: unknown,
  ): Promise<unknown> {
    const request: RequestEnvelope = {
      protocol: "dassie-rpc-01",
      type: "request",
      routeType: type,
      id: generateId(),
      path,
      input,
    }

    const callbackPromise = new Promise((resolve, reject) => {
      callbacks.set(request.id, (envelope) => {
        if (envelope.result.type === "success") {
          resolve(envelope.result.data)
        } else {
          reject(new Error(envelope.result.message))
        }
      })
    })

    await sendEnvelope(request)

    return await callbackPromise
  }

  function subscribe(
    path: string[],
    input: unknown,
    { onData, onError }: SubscriptionOptions<unknown, unknown>,
  ): () => void {
    const request: RequestEnvelope = {
      protocol: "dassie-rpc-01",
      type: "request",
      routeType: "subscription",
      id: generateId(),
      path,
      input,
    }

    subscriptions.set(request.id, {
      request,
      onData,
    })

    function dispose() {
      ;(async () => {
        const cancelRequest: CancelEnvelope = {
          protocol: "dassie-rpc-01",
          type: "cancel",
          id: request.id,
        }

        await sendEnvelope(cancelRequest)

        subscriptions.delete(request.id)
      })().catch((error: unknown) => {
        console.error("Error cancelling subscription", error)
      })
    }

    ;(async () => {
      const callbackPromise = new Promise((resolve, reject) => {
        callbacks.set(request.id, (envelope) => {
          if (envelope.result.type === "success") {
            resolve(envelope.result.data)
          } else {
            reject(new Error(envelope.result.message))
          }
        })
      })

      await sendEnvelope(request)

      await callbackPromise
    })().catch((error: unknown) => {
      if (onError) {
        onError(error)
      } else {
        console.error("Error subscribing", error)
      }
    })

    return dispose
  }

  async function handleProxyCall({ path, parameters }: ProxyCallbackOptions) {
    const routeVerb = path.pop()
    const routeType = routeVerb && ROUTE_HANDLER_TO_TYPE_MAP[routeVerb]

    if (!routeType) {
      throw new Error(
        `Invalid route verb "${routeVerb}", should be one of ${VALID_ROUTE_VERBS.join(", ")}`,
      )
    }

    return await (routeType === "subscription" ?
      createSubscription((onData) => subscribe(path, parameters[0], { onData }))
    : call(path, routeType, parameters[0]))
  }

  ;(async () => {
    for await (const value of received) {
      const serverMessage = transformer.parse(value)

      const envelopeParseResult = clientEnvelopeSchema.safeParse(serverMessage)

      if (!envelopeParseResult.success) {
        console.error("Invalid server message", {
          errors: envelopeParseResult.error.errors,
        })
        continue
      }

      const { data: envelope } = envelopeParseResult

      switch (envelope.type) {
        case "response": {
          const callback = callbacks.get(envelope.id)

          if (callback) {
            callback(envelope)

            callbacks.delete(envelope.id)
          } else {
            console.error("Unknown response ID", { id: envelope.id })
          }

          break
        }
        case "event": {
          const entry = subscriptions.get(envelope.id)

          if (entry) {
            try {
              for (const data of envelope.data) {
                entry.onData(data)
              }
            } catch (error) {
              // Error will be undefined if the stream was closed with no
              // "abort reason", which is normal.
              // eslint-disable-next-line @typescript-eslint/only-throw-error
              if (error !== undefined) throw error
            }
          } else {
            // We don't log an error when we receive an event for an unknown
            // subscription ID, as this can happen normally when we cancel
            // but the server hasn't processed the cancellation yet.
          }

          break
        }
        default: {
          throw new UnreachableCaseError(envelope)
        }
      }
    }
  })().catch((error: unknown) => {
    console.error("Uncaught RPC client error", error)
  })

  if (resets) {
    ;(async () => {
      for await (const _ of resets) {
        for (const entry of subscriptions.values()) {
          callbacks.set(entry.request.id, () => {
            // no-op
          })
          await sendEnvelope(entry.request)
        }
      }
    })().catch((error: unknown) => {
      console.error("Error handling resets", error)
    })
  }

  return {
    rpc: createRecursiveProxy(handleProxyCall) as DeriveClientRouter<TRouter>,
    query: (path, input) => call(path, "query", input),
    mutate: (path, input) => call(path, "mutation", input),
    subscribe,
    close,
  }
}
