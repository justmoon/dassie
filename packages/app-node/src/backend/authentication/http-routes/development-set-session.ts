import { z } from "zod"

import {
  createJsonResponse,
  parseBodyZod,
  setCookie,
} from "@dassie/lib-http-server"
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
