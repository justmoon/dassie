import { z } from "zod"

import { randomBytes } from "node:crypto"

import {
  UnauthorizedFailure,
  createJsonResponse,
} from "@dassie/lib-http-server"
import { createActor } from "@dassie/lib-reactive"

import { SESSION_COOKIE_NAME } from "../../../common/constants/cookie-name"
import { SEED_PATH_NODE_LOGIN } from "../../../common/constants/seed-paths"
import { NodePrivateKeySignal } from "../../crypto/computed/node-private-key"
import { getPrivateSeedAtPath } from "../../crypto/utils/seed-paths"
import { RestApiServiceActor } from "../../http-server/serve-rest-api"
import { COOKIE_MAX_AGE } from "../constants/cookie-lifetime"
import { SessionsStore } from "../database-stores/sessions"
import { SessionToken } from "../types/session-token"

export const RegisterLoginRouteActor = () =>
  createActor((sig) => {
    const api = sig.get(RestApiServiceActor)
    const sessions = sig.use(SessionsStore)

    if (!api) return

    api
      .post("/api/login")
      .body(
        z.object({
          loginAuthorizationSignature: z.string(),
        }),
      )
      .handler((request, response) => {
        const { loginAuthorizationSignature } = request.body

        const expectedLoginAuthorizationSignature = getPrivateSeedAtPath(
          sig.use(NodePrivateKeySignal).read(),
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

        response.cookie(SESSION_COOKIE_NAME, sessionToken, {
          httpOnly: true,
          secure: true,
          sameSite: "strict",
          expires: new Date(Date.now() + COOKIE_MAX_AGE),
        })

        return createJsonResponse({})
      })
  })
