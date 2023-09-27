import { z } from "zod"

import { randomBytes } from "node:crypto"

import { respondJson } from "@dassie/lib-http-server"
import { createActor } from "@dassie/lib-reactive"

import { SESSION_COOKIE_NAME } from "../../../common/constants/cookie-name"
import { SEED_PATH_NODE_LOGIN } from "../../../common/constants/seed-paths"
import {
  databaseConfigStore,
  hasNodeIdentity,
} from "../../config/database-config"
import { getPrivateSeedAtPath } from "../../crypto/utils/seed-paths"
import { restApiService } from "../../http-server/serve-rest-api"
import { serializeEd25519PrivateKey } from "../../utils/pem"
import { sessionsStore } from "../database-stores/sessions"
import { setupAuthorizationTokenSignal } from "../signals/setup-authorization-token"
import { SessionToken } from "../types/session-token"

export const registerSetupRoute = () =>
  createActor((sig) => {
    const api = sig.get(restApiService)
    const sessions = sig.use(sessionsStore)
    const config = sig.use(databaseConfigStore)
    const expectedSetupAuthorizationToken = sig
      .use(setupAuthorizationTokenSignal)
      .read()

    if (!api) return

    api
      .post("/api/setup")
      .body(
        z.object({
          setupAuthorizationToken: z.string(),
          rawDassieKeyHex: z.string(),
          loginAuthorizationSignature: z.string(),
        })
      )
      .handler((request, response) => {
        if (hasNodeIdentity(config.read())) {
          respondJson(response, 401, {
            error: "Node is already set up",
          })
          return
        }

        const {
          setupAuthorizationToken,
          rawDassieKeyHex,
          loginAuthorizationSignature,
        } = request.body

        if (setupAuthorizationToken !== expectedSetupAuthorizationToken) {
          respondJson(response, 401, {
            error: "Invalid setup authorization token",
          })
          return
        }

        const rawDassieKeyBuffer = Buffer.from(rawDassieKeyHex, "hex")

        const expectedLoginAuthorizationSignature = getPrivateSeedAtPath(
          rawDassieKeyBuffer,
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

        const dassieKey = serializeEd25519PrivateKey(rawDassieKeyBuffer)

        config.setNodeIdentity(dassieKey)

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
