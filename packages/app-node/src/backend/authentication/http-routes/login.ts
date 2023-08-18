import { z } from "zod"

import { randomBytes } from "node:crypto"

import { respondJson } from "@dassie/lib-http-server"
import { createActor } from "@dassie/lib-reactive"

import { SESSION_COOKIE_NAME } from "../../../common/constants/cookie-name"
import { SEED_PATH_NODE_LOGIN } from "../../../common/constants/seed-paths"
import { nodePrivateKeySignal } from "../../crypto/computed/node-private-key"
import { getPrivateSeedAtPath } from "../../crypto/utils/seed-paths"
import { restApiService } from "../../http-server/serve-rest-api"
import { sessionsStore } from "../database-stores/sessions"
import { SessionToken } from "../types/session-token"

export const registerLoginRoute = () =>
  createActor((sig) => {
    const api = sig.get(restApiService)
    const sessions = sig.use(sessionsStore)

    if (!api) return

    api
      .post("/api/login")
      .body(
        z.object({
          loginAuthorizationSignature: z.string(),
        })
      )
      .handler((request, response) => {
        const { loginAuthorizationSignature } = request.body

        const expectedLoginAuthorizationSignature = getPrivateSeedAtPath(
          sig.use(nodePrivateKeySignal).read(),
          SEED_PATH_NODE_LOGIN
        ).toString("hex")

        if (
          loginAuthorizationSignature !== expectedLoginAuthorizationSignature
        ) {
          respondJson(response, 401, {
            error: "Invalid login authorization signature",
          })
          return
        }

        const sessionToken = randomBytes(32).toString("hex") as SessionToken

        sessions.addSession(sessionToken)

        response.cookie(SESSION_COOKIE_NAME, sessionToken, {
          httpOnly: true,
          secure: true,
          sameSite: "strict",
        })

        respondJson(response, 200, {})
      })
  })
