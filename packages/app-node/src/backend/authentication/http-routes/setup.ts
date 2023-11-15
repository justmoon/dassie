import { z } from "zod"

import { randomBytes } from "node:crypto"

import {
  UnauthorizedFailure,
  createJsonResponse,
} from "@dassie/lib-http-server"
import { createActor } from "@dassie/lib-reactive"

import { SESSION_COOKIE_NAME } from "../../../common/constants/cookie-name"
import { SEED_PATH_NODE_LOGIN } from "../../../common/constants/seed-paths"
import {
  DatabaseConfigStore,
  hasNodeIdentity,
} from "../../config/database-config"
import { getPrivateSeedAtPath } from "../../crypto/utils/seed-paths"
import { HttpsRouter } from "../../http-server/serve-https"
import { serializeEd25519PrivateKey } from "../../utils/pem"
import { COOKIE_MAX_AGE } from "../constants/cookie-lifetime"
import { SessionsStore } from "../database-stores/sessions"
import { SetupAuthorizationTokenSignal } from "../signals/setup-authorization-token"
import { SessionToken } from "../types/session-token"

export const RegisterSetupRouteActor = () =>
  createActor((sig) => {
    const http = sig.use(HttpsRouter)
    const sessions = sig.use(SessionsStore)
    const config = sig.use(DatabaseConfigStore)
    const expectedSetupAuthorizationToken = sig
      .use(SetupAuthorizationTokenSignal)
      .read()

    http
      .post()
      .path("/api/setup")
      .bodySchema(
        z.object({
          setupAuthorizationToken: z.string(),
          rawDassieKeyHex: z.string(),
          loginAuthorizationSignature: z.string(),
        }),
      )
      .handler((request, response) => {
        if (hasNodeIdentity(config.read())) {
          return new UnauthorizedFailure("Node is already set up")
        }

        const {
          setupAuthorizationToken,
          rawDassieKeyHex,
          loginAuthorizationSignature,
        } = request.body

        if (setupAuthorizationToken !== expectedSetupAuthorizationToken) {
          return new UnauthorizedFailure("Invalid setup authorization token")
        }

        const rawDassieKeyBuffer = Buffer.from(rawDassieKeyHex, "hex")

        const expectedLoginAuthorizationSignature = getPrivateSeedAtPath(
          rawDassieKeyBuffer,
          SEED_PATH_NODE_LOGIN,
        ).toString("hex")

        if (
          loginAuthorizationSignature !== expectedLoginAuthorizationSignature
        ) {
          return new UnauthorizedFailure(
            "Invalid login authorization signature",
          )
        }

        const dassieKey = serializeEd25519PrivateKey(rawDassieKeyBuffer)

        config.setNodeIdentity(dassieKey)

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
