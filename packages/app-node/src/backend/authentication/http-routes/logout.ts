import { parse } from "cookie"

import { createJsonResponse } from "@dassie/lib-http-server"
import { createActor } from "@dassie/lib-reactive"

import { SESSION_COOKIE_NAME } from "../../../common/constants/cookie-name"
import { HttpsRouter } from "../../http-server/serve-https"
import { SessionsStore } from "../database-stores/sessions"
import { SessionToken } from "../types/session-token"

export const RegisterLogoutRouteActor = () =>
  createActor((sig) => {
    const http = sig.use(HttpsRouter)
    const sessions = sig.use(SessionsStore)

    http
      .post()
      .path("/api/logout")
      .handler(sig, (request, response) => {
        const cookies = parse(request.headers.cookie ?? "")
        const currentSessionToken = cookies[SESSION_COOKIE_NAME]
        if (currentSessionToken) {
          sessions.removeSession(currentSessionToken as SessionToken)
        }
        response.cookie(SESSION_COOKIE_NAME, "", {
          httpOnly: true,
          secure: true,
          sameSite: "strict",
          expires: new Date(0),
        })

        return createJsonResponse({})
      })
  })
