import type { Promisable } from "type-fest"

import { UnreachableCaseError, isFailure } from "@dassie/lib-type-utils"

import {
  type EventEnvelope,
  type RequestEnvelope,
  serverEnvelopeSchema,
} from "../common/envelope"
import type { Subscription } from "../common/subscription"
import type { Transformer } from "../common/types/transformer"
import type { AnyRouter, DeriveContextFromRouter } from "./router/router"

export interface ServerOptions {
  router: AnyRouter
  transformer?: Transformer
}

export interface InboundConnection {
  received: AsyncIterable<string>
  send: (data: string) => Promisable<void>
  closed: Promise<void>
}

export interface ConnectionOptions<TContext extends object> {
  context: TContext
  connection: InboundConnection
}

export interface Server<TContext extends object> {
  handleConnection(parameters: ConnectionOptions<TContext>): void
}

const MAX_SUBSCRIPTION_BATCH_SIZE = 100

export function createServer<TOptions extends ServerOptions>({
  router,
  transformer = JSON,
}: ServerOptions): Server<DeriveContextFromRouter<TOptions["router"]>> {
  return {
    handleConnection: ({ connection: { received, send, closed }, context }) => {
      ;(async () => {
        const subscriptions = new Map<string, () => void>()

        function handleRequest(envelope: RequestEnvelope) {
          async function respondSuccess(data: unknown) {
            const responseEnvelope = {
              protocol: "dassie-rpc-01",
              type: "response",
              id: envelope.id,
              result: {
                type: "success",
                data,
              },
            }

            await send(transformer.stringify(responseEnvelope))
          }

          async function respondError(message: string) {
            const responseEnvelope = {
              protocol: "dassie-rpc-01",
              type: "response",
              id: envelope.id,
              result: {
                type: "error",
                message,
              },
            }

            await send(transformer.stringify(responseEnvelope))
          }

          async function respondEvent(data: unknown[]) {
            const eventEnvelope: EventEnvelope = {
              protocol: "dassie-rpc-01",
              type: "event",
              id: envelope.id,
              data,
            }

            await send(transformer.stringify(eventEnvelope))
          }

          ;(async () => {
            const result = await router.call({
              path: envelope.path,
              type: envelope.routeType,
              input: envelope.input,
              context,
            })

            if (isFailure(result)) {
              await respondError(result.name)
              return
            }

            if (envelope.routeType === "subscription") {
              try {
                await respondSuccess(null)
              } catch (error) {
                console.error("responded success error", error)
              }

              const subscribe = result.data as Subscription<unknown>

              const queue: unknown[] = []
              let queuePromise: Promise<void> | undefined
              let isDisposed = false

              const dispose = subscribe(function handleEvent(data: unknown) {
                if (isDisposed) return
                ;(async () => {
                  queue.push(data)

                  if (queue.length >= MAX_SUBSCRIPTION_BATCH_SIZE) {
                    await queuePromise
                  }

                  if (queue.length === 1) {
                    queuePromise = new Promise((resolve, reject) => {
                      setTimeout(() => {
                        const batch = [...queue]
                        queue.length = 0
                        respondEvent(batch)
                          .then(resolve)
                          .catch((error: unknown) => {
                            // eslint-disable-next-line @typescript-eslint/prefer-promise-reject-errors
                            reject(error)
                          })
                      }, 0)
                    })
                  }
                })().catch((error: unknown) => {
                  console.error("Error handling subscription event", {
                    error,
                  })
                })
              })

              subscriptions.set(envelope.id, () => {
                isDisposed = true
                dispose()
              })
            } else {
              await respondSuccess(result.data)
            }
          })().catch(async (error: unknown) => {
            console.error("Error during RPC call", {
              error,
              path: envelope.path,
            })
            await respondError("Internal server error")
          })
        }

        closed
          .then(() => {
            for (const dispose of subscriptions.values()) {
              dispose()
            }
          })
          .catch((error: unknown) => {
            if (error) {
              console.error("Unhandled RPC server error during cleanup", {
                error,
              })
            }
          })

        for await (const value of received) {
          try {
            const clientMessage = transformer.parse(value)

            const envelopeParseResult =
              serverEnvelopeSchema.safeParse(clientMessage)

            if (!envelopeParseResult.success) {
              // If we can't parse the envelope, we unfortunately don't know the
              // request ID, so we can't tell the client.
              //
              // Best we can do is log the issue.
              console.error("Invalid envelope", {
                errors: envelopeParseResult.error.errors,
              })
              continue
            }

            const { data: envelope } = envelopeParseResult

            switch (envelope.type) {
              case "request": {
                handleRequest(envelope)
                break
              }

              case "cancel": {
                const dispose = subscriptions.get(envelope.id)
                if (dispose) {
                  dispose()
                  subscriptions.delete(envelope.id)
                }
                break
              }

              default: {
                new UnreachableCaseError(envelope)
              }
            }
          } catch (error: unknown) {
            console.error("Error processing RPC message", {
              error,
            })
          }
        }
      })().catch((error: unknown) => {
        console.error("Unhandled RPC server error", { error })
      })
    },
  }
}
