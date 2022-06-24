import { ZodType, z } from "zod"

import type { Serializable } from "node:child_process"
import { isNativeError } from "node:util/types"

import { createLogger } from "@xen-ilp/lib-logger"

const logger = createLogger("xen:dev:rpc-host")

export interface RpcRequest {
  id?: string | number | null | undefined
  method: string
  params: unknown[] | Record<string, unknown>
}

export interface RpcResult {
  id: string | number
  result: string | number | Record<string, unknown> | unknown[] | null
}

export interface RpcError {
  id: string | number
  error: { code: number; message: string }
}

export default class RpcHost<T extends RpcRequest> {
  private readonly schema: ZodType<T | RpcResult | RpcError>
  private readonly callbacks: Map<
    string,
    {
      resolve: (value?: Serializable) => void
      reject: (error: Error) => void
    }
  > = new Map()
  private unique = 0

  constructor(
    requestSchema: ZodType<T>,
    private handler: (request: T) => Promise<Serializable | null | undefined>,
    private send: (message: Serializable) => void
  ) {
    this.schema = z.union([
      requestSchema,
      z.object({
        id: z.union([z.string(), z.number()]),
        result: z.union([
          z.string(),
          z.number(),
          z.record(z.unknown()),
          z.array(z.unknown()),
          z.null(),
        ]),
      }),
      z.object({
        id: z.union([z.string(), z.number()]),
        error: z.object({ code: z.number(), message: z.string() }),
      }),
    ])
  }

  async call(
    method: string,
    parameters: unknown[] | Record<string, unknown> = []
  ): Promise<Serializable | undefined> {
    return new Promise<Serializable | undefined>((resolve, reject) => {
      const id = String(this.unique++)
      this.send({ id, method, params: parameters })
      this.callbacks.set(id, { resolve, reject })
    })
  }

  callNoReturn(
    method: string,
    parameters: unknown[] | Record<string, unknown> = []
  ) {
    this.send({ id: undefined, method, params: parameters })
  }

  async handleMessage(rawMessage: unknown) {
    let message
    try {
      message = this.schema.parse(rawMessage)
    } catch (error) {
      console.error("Was unable to parse packet:", rawMessage)
      throw error
    }

    if ("result" in message || "error" in message) {
      const callback = this.callbacks.get(String(message.id))
      if (callback) {
        const { resolve, reject } = callback
        if ("error" in message) {
          const match = /^([\w$]*Error): (.*)$/s.exec(message.error.message) as
            | [string, string, string]
            | null
          if (match) {
            const error = new Error(match[2])
            error.name = match[1]
            reject(error)
          } else {
            reject(new Error(message.error.message))
          }
        } else {
          resolve(message.result ?? undefined)
        }
        this.callbacks.delete(String(message.id))
      } else {
        logger.warn("unknown ID in RPC message", { message })
      }
    } else {
      try {
        // eslint-disable-next-line unicorn/no-null
        const result = (await this.handler(message)) ?? null
        this.send({
          id: message.id,
          result,
        })
      } catch (error) {
        const errorMessage = isNativeError(error) ? error.stack : String(error)
        this.send({
          id: message.id,
          error: {
            code: 0,
            message: errorMessage,
          },
        })
      }
    }
  }
}
