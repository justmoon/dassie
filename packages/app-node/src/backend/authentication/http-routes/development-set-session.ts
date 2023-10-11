import { z } from "zod"

import { respondJson } from "@dassie/lib-http-server"
import { createActor } from "@dassie/lib-reactive"

import { SESSION_COOKIE_NAME } from "../../../common/constants/cookie-name"
import { RestApiServiceActor } from "../../http-server/serve-rest-api"
import { SessionToken } from "../types/session-token"

export const RegisterDevelopmentSetSessionRouteActor = () =>
  createActor((sig) => {
    if (!import.meta.env.DEV) return

    const api = sig.get(RestApiServiceActor)

    if (!api) return

    api
      .post("/api/_dev/set-session")
      .body(
        z.object({
          sessionToken: z
            .string()
            .refine((_token): _token is SessionToken => true),
        }),
      )
      .handler((request, response) => {
        const { sessionToken } = request.body

        response.cookie(SESSION_COOKIE_NAME, sessionToken, {
          httpOnly: true,
          secure: true,
          sameSite: "strict",
        })

        respondJson(response, 200, {})
      })
  })
