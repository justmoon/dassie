import { uint8ArrayToHex } from "uint8array-extras"
import { z } from "zod"

import {
  UnauthorizedFailure,
  createJsonResponse,
  parseBodyZod,
  setCookie,
} from "@dassie/lib-http-server"
import { createActor } from "@dassie/lib-reactive"
import { serializeEd25519Key } from "@dassie/lib-x509"

import type { DassieReactor } from "../../base/types/dassie-base"
import {
  DatabaseConfigStore,
  hasNodeIdentity,
} from "../../config/database-config"
import { SESSION_COOKIE_NAME } from "../../constants/cookie-name"
import { SEED_PATH_NODE_LOGIN } from "../../constants/seed-paths"
import { getPrivateSeedAtPath } from "../../crypto/utils/seed-paths"
import { HttpsRouter } from "../../http-server/values/https-router"
import { COOKIE_MAX_AGE_SECONDS } from "../constants/cookie-lifetime"
import { SessionsStore } from "../database-stores/sessions"
import { SetupAuthorizationTokenSignal } from "../signals/setup-authorization-token"
import type { SessionToken } from "../types/session-token"

export const RegisterSetupRouteActor = (reactor: DassieReactor) => {
  const { crypto } = reactor.base
  const http = reactor.use(HttpsRouter)
  const sessions = reactor.use(SessionsStore)
  const config = reactor.use(DatabaseConfigStore)
  const setupAuthorizationTokenSignal = reactor.use(
    SetupAuthorizationTokenSignal,
  )

  return createActor((sig) => {
    http
      .post()
      .path("/api/setup")
      .use(
        parseBodyZod(
          z.object({
            setupAuthorizationToken: z.string(),
            rawDassieKeyHex: z.string(),
            loginAuthorizationSignature: z.string(),
          }),
        ),
      )
      .handler(sig, ({ body, response: { headers } }) => {
        if (hasNodeIdentity(config.read())) {
          return new UnauthorizedFailure("Node is already set up")
        }

        const expectedSetupAuthorizationToken =
          setupAuthorizationTokenSignal.read()

        const {
          setupAuthorizationToken,
          rawDassieKeyHex,
          loginAuthorizationSignature,
        } = body

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

        const dassieKey = serializeEd25519Key(rawDassieKeyBuffer, "private")

        config.act.setNodeIdentity(dassieKey)

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
