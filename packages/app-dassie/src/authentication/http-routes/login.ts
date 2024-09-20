import { uint8ArrayToHex } from "uint8array-extras"
import { z } from "zod"

import {
  UnauthorizedFailure,
  createJsonResponse,
  parseBodyZod,
  setCookie,
} from "@dassie/lib-http-server"
import { createActor } from "@dassie/lib-reactive"

import type { DassieReactor } from "../../base/types/dassie-base"
import { SESSION_COOKIE_NAME } from "../../constants/cookie-name"
import { SEED_PATH_NODE_LOGIN } from "../../constants/seed-paths"
import { NodePrivateKeySignal } from "../../crypto/computed/node-private-key"
import { getPrivateSeedAtPath } from "../../crypto/utils/seed-paths"
import { HttpsRouter } from "../../http-server/values/https-router"
import { COOKIE_MAX_AGE_SECONDS } from "../constants/cookie-lifetime"
import { SessionsStore } from "../database-stores/sessions"
import type { SessionToken } from "../types/session-token"

export const RegisterLoginRouteActor = (reactor: DassieReactor) => {
  const { crypto } = reactor.base

  return createActor((sig) => {
    const http = sig.reactor.use(HttpsRouter)
    const sessions = sig.reactor.use(SessionsStore)

    http
      .post()
      .path("/api/login")
      .use(
        parseBodyZod(
          z.object({
            loginAuthorizationSignature: z.string(),
          }),
        ),
      )
      .handler(sig, ({ body, response: { headers } }) => {
        const { loginAuthorizationSignature } = body

        const expectedLoginAuthorizationSignature = getPrivateSeedAtPath(
          sig.read(NodePrivateKeySignal),
          SEED_PATH_NODE_LOGIN,
        ).toString("hex")

        if (
          loginAuthorizationSignature !== expectedLoginAuthorizationSignature
        ) {
          return new UnauthorizedFailure(
            "Invalid login authorization signature",
          )
        }

        const sessionToken = uint8ArrayToHex(
          crypto.getRandomBytes(32),
        ) as SessionToken

        sessions.act.addSession(sessionToken)

        setCookie(headers, {
          name: SESSION_COOKIE_NAME,
          value: sessionToken,
          maxAge: COOKIE_MAX_AGE_SECONDS,
        })

        return createJsonResponse({})
      })
  })
}
