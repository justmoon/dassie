import { uint8ArrayToBase64 } from "uint8array-extras"
import { z } from "zod"

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
    return [...sig.read(BtpTokensStore)]
  }),
  addBtpToken: protectedProcedure.mutation(({ ctx: { sig } }) => {
    const { random } = sig.reactor.base
    const token = uint8ArrayToBase64(random.randomBytes(16), {
      urlSafe: true,
    }) as BtpToken
    const btpTokensStore = sig.reactor.use(BtpTokensStore)
    btpTokensStore.addToken(token)

    return token
  }),
  removeBtpToken: protectedProcedure
    .input(btpTokenSchema)
    .mutation(({ input: token, ctx: { sig } }) => {
      const btpTokensStore = sig.reactor.use(BtpTokensStore)
      btpTokensStore.removeToken(token)
    }),
})
