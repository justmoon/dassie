export interface CookieOptions {
  name: string
  value: string
  maxAge?: number | undefined
  partitioned?: boolean | undefined
}

export const setCookie = (headers: Headers, cookie: CookieOptions) => {
  if (!cookie.name.startsWith("__Host-")) {
    throw new Error(
      `Cookie name must start with "__Host-". Search online for "cookie prefixes" to learn more. Received "${cookie.name}"`,
    )
  }

  let cookieHeader = `${cookie.name}=${cookie.value}; Path=/; HttpOnly; Secure; SameSite=Strict`

  if (typeof cookie.maxAge === "number") {
    if (cookie.maxAge < 0) {
      throw new TypeError(
        `Cookie maxAge must be a positive number. Received "${cookie.maxAge}"`,
      )
    }

    cookieHeader += `; Max-Age=${cookie.maxAge}`
  }

  if (cookie.partitioned) {
    cookieHeader += `; Partitioned`
  }

  headers.append("Set-Cookie", cookieHeader)
}

export const clearCookie = (headers: Headers, name: string) => {
  setCookie(headers, {
    name,
    value: "",
    maxAge: 0,
  })
}
