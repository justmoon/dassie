import { z } from "zod"

import { randomBytes } from "node:crypto"

import {
  UnauthorizedFailure,
  createJsonResponse,
  setCookie,
} from "@dassie/lib-http-server"
import { createActor } from "@dassie/lib-reactive"

import { SESSION_COOKIE_NAME } from "../../../common/constants/cookie-name"
import { SEED_PATH_NODE_LOGIN } from "../../../common/constants/seed-paths"
import { NodePrivateKeySignal } from "../../crypto/computed/node-private-key"
import { getPrivateSeedAtPath } from "../../crypto/utils/seed-paths"
import { HttpsRouter } from "../../http-server/serve-https"
import { COOKIE_MAX_AGE_SECONDS } from "../constants/cookie-lifetime"
import { SessionsStore } from "../database-stores/sessions"
import { SessionToken } from "../types/session-token"

export const RegisterLoginRouteActor = () =>
  createActor((sig) => {
    const http = sig.reactor.use(HttpsRouter)
    const sessions = sig.reactor.use(SessionsStore)

    if (!http) return

    http
      .post()
      .path("/api/login")
      .bodySchemaZod(
        z.object({
          loginAuthorizationSignature: z.string(),
        }),
      )
      .handler(sig, (request, response) => {
        const { loginAuthorizationSignature } = request.body

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

        const sessionToken = randomBytes(32).toString("hex") as SessionToken

        sessions.addSession(sessionToken)

        setCookie(response, {
          name: SESSION_COOKIE_NAME,
          value: sessionToken,
          maxAge: COOKIE_MAX_AGE_SECONDS,
        })

        return createJsonResponse({})
      })
  })
