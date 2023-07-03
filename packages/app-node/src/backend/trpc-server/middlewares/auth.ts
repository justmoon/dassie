import { TRPCError } from "@trpc/server"

import { trpc } from "../trpc-context"

const isAuthenticated = trpc.middleware((options) => {
  const { ctx } = options
  if (!ctx.user) {
    throw new TRPCError({
      code: "UNAUTHORIZED",
      message: "Only authenticated users may access this endpoint",
    })
  }
  return options.next({
    ctx,
  })
})

export const protectedProcedure = trpc.procedure.use(isAuthenticated)
