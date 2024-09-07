/* eslint-disable unicorn/no-hex-escape, unicorn/better-regex -- In this case, the regex is actually more readable without these rules.*/
import type { Middleware } from "../router"

export type Cookies = Record<string, string>

export const COOKIE_TOKEN_REGEX =
  /^[\x21\x23-\x27\x2A-\x2B\x2D-\x2E\x30-\x39\x41-\x5A\x5E-\x7A\x7C\x7E]+$/

export const COOKIE_VALUE_REGEX =
  /^[\x21\x23-\x2B\x2D-\x3A\x3C-\x5B\x5D-\x7E]*$/

export const cookie = (({ request }: { request: Request }) => {
  const cookies: Cookies = {}

  const cookieHeader = request.headers.get("cookie")
  if (cookieHeader) {
    for (const cookie of cookieHeader.split(";")) {
      const parts = cookie.split("=").map((part) => part.trim()) as
        | [string]
        | [string, string]

      if (parts[1] === undefined) continue

      const name = parts[0]
      const value =
        parts[1].startsWith('"') && parts[1].endsWith('"') ?
          parts[1].slice(1, -1)
        : parts[1]

      if (
        COOKIE_TOKEN_REGEX.test(name) &&
        cookies[name] === undefined &&
        COOKIE_VALUE_REGEX.test(value)
      ) {
        cookies[name] = value
      }
    }
  }

  return { cookies }
}) satisfies Middleware<{}, { cookies: Cookies }>
