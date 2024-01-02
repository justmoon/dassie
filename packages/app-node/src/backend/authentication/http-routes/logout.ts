import { parse } from "cookie"

import { clearCookie, createJsonResponse } from "@dassie/lib-http-server"
import { createActor } from "@dassie/lib-reactive"

import { SESSION_COOKIE_NAME } from "../../../common/constants/cookie-name"
import { HttpsRouter } from "../../http-server/serve-https"
import { SessionsStore } from "../database-stores/sessions"
import { SessionToken } from "../types/session-token"

export const RegisterLogoutRouteActor = () =>
  createActor((sig) => {
    const http = sig.reactor.use(HttpsRouter)
    const sessions = sig.reactor.use(SessionsStore)

    http
      .post()
      .path("/api/logout")
      .handler(sig, (request, response) => {
        const cookies = parse(request.headers.cookie ?? "")
        const currentSessionToken = cookies[SESSION_COOKIE_NAME]
        if (currentSessionToken) {
          sessions.removeSession(currentSessionToken as SessionToken)
        }

        clearCookie(response, SESSION_COOKIE_NAME)

        return createJsonResponse({})
      })
  })
