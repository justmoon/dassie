import { parse } from "cookie"

import { respondJson } from "@dassie/lib-http-server"
import { createActor } from "@dassie/lib-reactive"

import { SESSION_COOKIE_NAME } from "../../../common/constants/cookie-name"
import { RestApiServiceActor } from "../../http-server/serve-rest-api"
import { SessionsStore } from "../database-stores/sessions"
import { SessionToken } from "../types/session-token"

export const RegisterLogoutRouteActor = () =>
  createActor((sig) => {
    const api = sig.get(RestApiServiceActor)
    const sessions = sig.use(SessionsStore)
    if (!api) return

    api.post("/api/logout").handler((request, response) => {
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

      respondJson(response, 200, {})
    })
  })
