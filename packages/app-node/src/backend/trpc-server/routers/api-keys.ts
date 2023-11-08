import { z } from "zod"

import { randomBytes } from "node:crypto"

import { BtpTokensStore } from "../../api-keys/stores/btp-tokens"
import { protectedProcedure } from "../middlewares/auth"
import { trpc } from "../trpc-context"

export const apiKeysRouter = trpc.router({
  getCurrentKeys: protectedProcedure.query(({ ctx: { sig } }) => {
    const btpTokensStore = sig.use(BtpTokensStore)
    return [...btpTokensStore.read()]
  }),
  addBtpToken: protectedProcedure.mutation(({ ctx: { sig } }) => {
    const token = randomBytes(16).toString("base64url")
    const btpTokensStore = sig.use(BtpTokensStore)
    btpTokensStore.addToken(token)

    return token
  }),
  removeBtpToken: protectedProcedure
    .input(z.string())
    .mutation(({ input: token, ctx: { sig } }) => {
      const btpTokensStore = sig.use(BtpTokensStore)
      btpTokensStore.removeToken(token)
    }),
})
