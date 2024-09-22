import { z } from "zod"

import { COOKIE_MAX_AGE_SECONDS } from "@dassie/app-dassie/src/authentication/constants/cookie-lifetime"
import type { SessionToken } from "@dassie/app-dassie/src/authentication/types/session-token"
import { SESSION_COOKIE_NAME } from "@dassie/app-dassie/src/constants/cookie-name"
import { HttpsRouter } from "@dassie/app-dassie/src/http-server/values/https-router"
import {
  createJsonResponse,
  parseBodyZod,
  setCookie,
} from "@dassie/lib-http-server"
import { createActor } from "@dassie/lib-reactive"

export const RegisterDevelopmentSetSessionRouteActor = () =>
  createActor((sig) => {
    const http = sig.reactor.use(HttpsRouter)

    http
      .post()
      .path("/api/_dev/set-session")
      .use(
        parseBodyZod(
          z.object({
            sessionToken: z
              .string()
              .refine((_token): _token is SessionToken => true),
          }),
        ),
      )
      .handler(sig, ({ body, response: { headers } }) => {
        const { sessionToken } = body

        setCookie(headers, {
          name: SESSION_COOKIE_NAME,
          value: sessionToken,
          maxAge: COOKIE_MAX_AGE_SECONDS,
        })

        return createJsonResponse({})
      })
  })
