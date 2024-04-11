import { negotiate } from "@fastify/accept-negotiator"

import type { BaseRequestContext } from "./types/context"

export const handleError = (
  { request }: Pick<BaseRequestContext, "request">,
  error: unknown,
): Response => {
  const mediaType = negotiate(request.headers.get("accept") ?? "", [
    "application/json",
    "text/html",
  ])

  console.error("error in http request handler", { error })

  switch (mediaType) {
    case "application/json": {
      return new Response(JSON.stringify({ error: "Internal Server Error" }), {
        status: 500,
        headers: { "Content-Type": "application/json" },
      })
    }
    case "text/html": {
      return new Response("<h1>Internal Server Error</h1>", {
        status: 500,
        headers: { "Content-Type": "text/html" },
      })
    }
    default: {
      return new Response("Internal Server Error", { status: 500 })
    }
  }
}
