/* eslint-disable @typescript-eslint/no-explicit-any */

/* eslint-disable @typescript-eslint/no-unsafe-assignment */
import { TRPCClientError, TRPCLink } from "@trpc/client"
import type { AnyRouter } from "@trpc/server"
import { observable } from "@trpc/server/observable"

import { Socket } from "node:net"
import { createInterface } from "node:readline"

import type { TRPCSocketRequest, TRPCSocketResponse } from "../common/types"

export interface SocketLinkOptions {
  socket: Socket
}

export const socketLink = <TRouter extends AnyRouter>(
  options: SocketLinkOptions
): TRPCLink<TRouter> => {
  return (runtime) => {
    const { socket } = options

    const socketLineReader = createInterface({
      input: socket,
    })

    return ({ op }) => {
      return observable((observer) => {
        const disposers: (() => void)[] = []

        const { id, type, path } = op

        try {
          const input = runtime.transformer.serialize(op.input)

          const onDisconnect = () => {
            observer.error(new TRPCClientError("Port disconnected prematurely"))
          }

          socket.addListener("close", onDisconnect)
          disposers.push(() => socket.removeListener("close", onDisconnect))

          const onMessage = (line: string) => {
            const message = JSON.parse(line) as TRPCSocketResponse
            if (!("id" in message) || message.id == null) return
            if (id !== message.id) return

            if ("error" in message) {
              const error = runtime.transformer.deserialize(message.error)
              observer.error(TRPCClientError.from({ ...message, error }))
              return
            }

            observer.next({
              result: {
                ...message.result,
                ...(message.result.type === "data" && {
                  type: "data",
                  data: runtime.transformer.deserialize(message.result.data),
                }),
              } as any,
            })

            if (type !== "subscription" || message.result.type === "stopped") {
              observer.complete()
            }
          }

          socketLineReader.addListener("line", onMessage)
          disposers.push(() => socketLineReader.close())

          socket.write(
            JSON.stringify({
              id,
              jsonrpc: "2.0",
              method: type,
              params: { path, input },
            } as TRPCSocketRequest) + "\n"
          )
        } catch (error) {
          observer.error(
            new TRPCClientError(
              error instanceof Error ? error.message : "Unknown error"
            )
          )
        }

        return () => {
          for (const disposer of disposers) disposer()
          if (type === "subscription") {
            socket.write(
              JSON.stringify({
                id,
                jsonrpc: "2.0",
                method: "subscription.stop",
              } as TRPCSocketRequest) + "\n"
            )
          }
        }
      })
    }
  }
}
