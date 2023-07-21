/* eslint-disable @typescript-eslint/no-explicit-any */

/* eslint-disable @typescript-eslint/no-unsafe-call */

/* eslint-disable @typescript-eslint/no-unsafe-member-access */

/* eslint-disable @typescript-eslint/no-unsafe-assignment */
import {
  AnyRouter,
  CombinedDataTransformer,
  ProcedureType,
  TRPCError,
  inferRouterContext,
} from "@trpc/server"
import { Unsubscribable, isObservable } from "@trpc/server/observable"
import { getErrorShape } from "@trpc/server/shared"
import type { Promisable } from "type-fest"

import { Socket } from "node:net"
import { createInterface } from "node:readline"

import type {
  SocketOnErrorFunction,
  TRPCSocketRequest,
  TRPCSocketResponse,
} from "../common/types"
import { getErrorFromUnknown } from "./errors"

export interface SocketCreateContextOptions {
  socket: Socket
}

export interface CreateSocketHandlerOptions<TRouter extends AnyRouter> {
  router: TRouter
  createContext?: (
    options: SocketCreateContextOptions
  ) => Promisable<inferRouterContext<TRouter>>
  onError?: SocketOnErrorFunction<TRouter>
}

export const createSocketHandler = <TRouter extends AnyRouter>(
  options: CreateSocketHandlerOptions<TRouter>
) => {
  const { router, createContext, onError } = options
  const { transformer } = router._def._config as {
    transformer: CombinedDataTransformer
  }

  return (socket: Socket) => {
    const subscriptions = new Map<number | string, Unsubscribable>()
    const disposers: (() => void)[] = []

    const onDisconnect = () => {
      for (const disposer of disposers) disposer()
    }

    socket.addListener("close", onDisconnect)
    disposers.push(() => socket.removeListener("close", onDisconnect))

    const onMessage = async (line: string) => {
      const message = JSON.parse(line) as TRPCSocketRequest
      if (!("id" in message) || message.id == null) return

      const { id, method } = message

      const sendResponse = (response: TRPCSocketResponse) => {
        socket.write(
          JSON.stringify({
            id,
            jsonrpc: "2.0",
            ...response,
          } as TRPCSocketResponse) + "\n"
        )
      }

      let parameters: { path: string; input: unknown } | undefined
      let input: any
      let context: any

      try {
        if (method === "subscription.stop") {
          const subscription = subscriptions.get(id)
          if (subscription) {
            subscription.unsubscribe()
            sendResponse({
              result: {
                type: "stopped",
              },
            })
          }
          subscriptions.delete(id)
          return
        }

        // eslint-disable-next-line unicorn/consistent-destructuring
        parameters = message.params

        input = transformer.input.deserialize(parameters.input)

        context = await createContext?.({ socket })
        const caller = router.createCaller(context)

        const segments = parameters.path.split(".")
        let procedureFunction = caller as any
        for (const segment of segments) {
          procedureFunction = procedureFunction[segment]
        }

        const result = await procedureFunction(input)

        if (method !== "subscription") {
          const data = transformer.output.serialize(result)
          sendResponse({
            result: {
              type: "data",
              data,
            },
          })
          return
        }

        if (!isObservable(result)) {
          throw new TRPCError({
            message: "Subscription ${params.path} did not return an observable",
            code: "INTERNAL_SERVER_ERROR",
          })
        }

        const subscription = result.subscribe({
          next: (data) => {
            sendResponse({
              result: {
                type: "data",
                data,
              },
            })
          },
          error: (cause) => {
            const error = getErrorFromUnknown(cause)

            onError?.({
              error,
              type: method,
              path: parameters?.path,
              input,
              ctx: context,
              socket,
            })

            sendResponse({
              error: getErrorShape({
                config: router._def._config,
                error,
                type: method,
                path: parameters?.path,
                input,
                ctx: context,
              }),
            })
          },
          complete: () => {
            sendResponse({
              result: {
                type: "stopped",
              },
            })
          },
        })

        if (subscriptions.has(id)) {
          subscription.unsubscribe()
          sendResponse({
            result: {
              type: "stopped",
            },
          })
          throw new TRPCError({
            message: `Duplicate id ${id}`,
            code: "BAD_REQUEST",
          })
        }
        disposers.push(() => subscription.unsubscribe())

        subscriptions.set(id, subscription)

        sendResponse({
          result: {
            type: "started",
          },
        })
        return
      } catch (error_) {
        const error = getErrorFromUnknown(error_)

        onError?.({
          error,
          type: method as ProcedureType,
          path: parameters?.path,
          input,
          ctx: context,
          socket,
        })

        sendResponse({
          error: getErrorShape({
            config: router._def._config,
            error,
            type: method as ProcedureType,
            path: parameters?.path,
            input,
            ctx: context,
          }),
        })
      }
    }

    const socketLineReader = createInterface({
      input: socket,
    })
    socketLineReader.addListener("line", (line) => {
      onMessage(line).catch((error) => {
        console.error("failed to handle ipc message", { error })
      })
    })
    disposers.push(() => socketLineReader.close())
  }
}
