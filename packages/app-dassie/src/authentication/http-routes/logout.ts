import { parse } from "cookie"

import { clearCookie, createJsonResponse } from "@dassie/lib-http-server"
import { createActor } from "@dassie/lib-reactive"

import { SESSION_COOKIE_NAME } from "../../constants/cookie-name"
import { HttpsRouter } from "../../http-server/values/https-router"
import { SessionsStore } from "../database-stores/sessions"
import type { SessionToken } from "../types/session-token"

export const RegisterLogoutRouteActor = () =>
  createActor((sig) => {
    const http = sig.reactor.use(HttpsRouter)
    const sessions = sig.reactor.use(SessionsStore)

    http
      .post()
      .path("/api/logout")
      .handler(sig, ({ request, response: { headers } }) => {
        const cookies = parse(request.headers.get("cookie") ?? "")
        const currentSessionToken = cookies[SESSION_COOKIE_NAME]
        if (currentSessionToken) {
          sessions.act.removeSession(currentSessionToken as SessionToken)
        }

        clearCookie(headers, SESSION_COOKIE_NAME)

        return createJsonResponse({})
      })
  })
