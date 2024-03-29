import { z } from "zod"

import { createJsonResponse, setCookie } from "@dassie/lib-http-server"
import { createActor } from "@dassie/lib-reactive"

import { SESSION_COOKIE_NAME } from "../../../common/constants/cookie-name"
import { HttpsRouter } from "../../http-server/serve-https"
import { COOKIE_MAX_AGE_SECONDS } from "../constants/cookie-lifetime"
import { SessionToken } from "../types/session-token"

export const RegisterDevelopmentSetSessionRouteActor = () =>
  createActor((sig) => {
    if (!import.meta.env.DEV) return

    const http = sig.reactor.use(HttpsRouter)

    http
      .post()
      .path("/api/_dev/set-session")
      .bodySchemaZod(
        z.object({
          sessionToken: z
            .string()
            .refine((_token): _token is SessionToken => true),
        }),
      )
      .handler(sig, (request, response) => {
        const { sessionToken } = request.body

        setCookie(response, {
          name: SESSION_COOKIE_NAME,
          value: sessionToken,
          maxAge: COOKIE_MAX_AGE_SECONDS,
        })

        return createJsonResponse({})
      })
  })
