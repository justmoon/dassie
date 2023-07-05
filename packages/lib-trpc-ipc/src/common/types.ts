import {
  AnyRouter,
  ProcedureType,
  TRPCError,
  inferRouterContext,
} from "@trpc/server"
import type {
  TRPCClientOutgoingMessage,
  TRPCErrorResponse,
  TRPCRequest,
  TRPCResultMessage,
} from "@trpc/server/rpc"

import { Socket } from "node:net"

export interface TRPCSocketRequest {
  trpc: TRPCRequest | TRPCClientOutgoingMessage
}

export interface TRPCSocketSuccessResponse {
  trpc: TRPCResultMessage<unknown>
}

export interface TRPCSocketErrorResponse {
  trpc: TRPCErrorResponse
}

export type TRPCSocketResponse =
  | TRPCSocketSuccessResponse
  | TRPCSocketErrorResponse

export type SocketOnErrorFunction<TRouter extends AnyRouter> = (options: {
  error: TRPCError
  type: ProcedureType | "unknown"
  path: string | undefined
  socket: Socket
  input: unknown
  ctx: inferRouterContext<TRouter> | undefined
}) => void
