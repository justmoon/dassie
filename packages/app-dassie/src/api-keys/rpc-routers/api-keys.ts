import { uint8ArrayToBase64 } from "uint8array-extras"
import { z } from "zod"

import { createRouter } from "@dassie/lib-rpc/server"

import { protectedRoute } from "../../rpc-server/route-types/protected"
import { BtpTokensStore } from "../database-stores/btp-tokens"
import type { BtpToken } from "../types/btp-token"

const btpTokenSchema = z
  .string()
  .length(22)
  .transform((token) => token as BtpToken)

export const apiKeysRouter = createRouter({
  getCurrentKeys: protectedRoute.query(({ context: { sig } }) => {
    return [...sig.read(BtpTokensStore)]
  }),
  addBtpToken: protectedRoute.mutation(({ context: { sig } }) => {
    const { crypto } = sig.reactor.base
    const token = uint8ArrayToBase64(crypto.getRandomBytes(16), {
      urlSafe: true,
    }) as BtpToken
    const btpTokensStore = sig.reactor.use(BtpTokensStore)
    btpTokensStore.act.addToken(token)

    return token
  }),
  removeBtpToken: protectedRoute
    .input(btpTokenSchema)
    .mutation(({ input: token, context: { sig } }) => {
      const btpTokensStore = sig.reactor.use(BtpTokensStore)
      btpTokensStore.act.removeToken(token)
    }),
})
