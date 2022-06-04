import { ZodType, z } from "zod"

import type { Serializable } from "node:child_process"

export interface RpcRequest {
  id: string | number | null | undefined
  method: string
  params: unknown[] | Record<string, unknown>
}

export interface RpcResult {
  id: string | number
  result: string | number | Record<string, unknown> | Array<unknown> | null
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
      resolve: (value: Serializable | null) => void
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
    parameters: unknown[] | Record<string, unknown>
  ): Promise<Serializable | null> {
    return new Promise<Serializable | null>((resolve, reject) => {
      const id = String(this.unique++)
      this.send({ id, method, params: parameters })
      this.callbacks.set(id, { resolve, reject })
    })
  }

  async handleMessage(rawMessage: unknown) {
    const message = this.schema.parse(rawMessage)

    if ("result" in message || "error" in message) {
      const callback = this.callbacks.get(String(message.id))
      if (callback) {
        const { resolve, reject } = callback
        if ("error" in message) {
          reject(new Error(message.error.message))
        } else {
          resolve(message.result)
        }
        this.callbacks.delete(String(message.id))
      } else {
        console.error(`Unknown ID in RPC message: ${message}`)
      }
    } else {
      try {
        // eslint-disable-next-line unicorn/no-null
        const result = (await this.handler(message)) ?? null
        this.send({
          id: message.id,
          result: result,
        })
      } catch (error) {
        this.send({
          id: message.id,
          error: {
            code: 0,
            message: String(error),
          },
        })
      }
    }
  }
}
