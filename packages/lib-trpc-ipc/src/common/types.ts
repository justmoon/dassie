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

export type TRPCSocketRequest = TRPCRequest | TRPCClientOutgoingMessage

export type TRPCSocketSuccessResponse = TRPCResultMessage<unknown>

export type TRPCSocketErrorResponse = TRPCErrorResponse

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
