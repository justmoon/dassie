import { z } from "zod"

import { randomBytes } from "node:crypto"

import { protectedProcedure } from "../../trpc-server/middlewares/auth"
import { trpc } from "../../trpc-server/trpc-context"
import { BtpTokensStore } from "../database-stores/btp-tokens"
import { BtpToken } from "../types/btp-token"

const btpTokenSchema = z
  .string()
  .length(22)
  .transform((token) => token as BtpToken)

export const apiKeysRouter = trpc.router({
  getCurrentKeys: protectedProcedure.query(({ ctx: { sig } }) => {
    const btpTokensStore = sig.use(BtpTokensStore)
    return [...btpTokensStore.read()]
  }),
  addBtpToken: protectedProcedure.mutation(({ ctx: { sig } }) => {
    const token = randomBytes(16).toString("base64url") as BtpToken
    const btpTokensStore = sig.use(BtpTokensStore)
    btpTokensStore.addToken(token)

    return token
  }),
  removeBtpToken: protectedProcedure
    .input(btpTokenSchema)
    .mutation(({ input: token, ctx: { sig } }) => {
      const btpTokensStore = sig.use(BtpTokensStore)
      btpTokensStore.removeToken(token)
    }),
})
