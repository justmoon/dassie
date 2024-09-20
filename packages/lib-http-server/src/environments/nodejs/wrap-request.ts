import {
  type IncomingMessage,
  STATUS_CODES,
  type ServerResponse,
} from "node:http"
import type { Socket } from "node:net"
import { ReadableStream as NodeReadableStream } from "node:stream/web"

export interface NodejsWrapperOptions extends RequestOptions {
  errorHandler?: ((error: unknown) => void) | undefined
}

export function wrapHandlerForNodejs(
  handler: (request: Request) => Promise<Response>,
  {
    errorHandler = handleErrorDefault,
    ...requestOptions
  }: NodejsWrapperOptions,
): (nodeRequest: IncomingMessage, nodeResponse: ServerResponse) => void {
  return (nodeRequest, nodeResponse) => {
    ;(async () => {
      const request = convertFromNodejsRequest(nodeRequest, requestOptions)
      const response = await handler(request)
      await writeToNodejsResponse(response, nodeResponse)
    })().catch(errorHandler)
  }
}

interface RequestOptions {
  protocol: "http" | "https"
  hostname: string
}

const VERBS_WITH_NO_BODY = new Set([
  "GET",
  "HEAD",
  "OPTIONS",
  "TRACE",
  "DELETE",
])

export function convertFromNodejsRequest(
  nodeRequest: IncomingMessage,
  { protocol, hostname }: RequestOptions,
): Request {
  const method = nodeRequest.method ?? "GET"
  const url = `${protocol}://${hostname}${nodeRequest.url}`
  const hasBody = !VERBS_WITH_NO_BODY.has(method)
  const requestOptions = {
    method,
    headers: convertFromNodejsRequestHeaders(nodeRequest.headers),
    ...(hasBody ?
      {
        body: NodeReadableStream.from<Uint8Array>(
          nodeRequest,
          // eslint-disable-next-line n/no-unsupported-features/node-builtins
        ) as ReadableStream<Uint8Array>,
        duplex: "half" as const,
      }
    : { body: null }),
  }

  return new Request(url, requestOptions)
}

function convertFromNodejsRequestHeaders(
  nodeHeaders: IncomingMessage["headers"],
): Headers {
  const headers = new Headers()

  for (const [key, value] of Object.entries(nodeHeaders)) {
    if (value === undefined) continue

    if (Array.isArray(value)) {
      for (const item of value) {
        headers.append(key, item)
      }

      continue
    }

    headers.append(key, value)
  }

  return headers
}

export async function writeToNodejsResponse(
  response: Response,
  nodeResponse: ServerResponse,
) {
  const { status, headers, body } = response
  nodeResponse.writeHead(status, convertToNodejsResponseHeaders(headers))

  if (!body) {
    nodeResponse.end()
    return
  }

  try {
    const reader = body.getReader()
    nodeResponse.on("close", () => {
      reader.cancel().catch((error: unknown) => {
        console.error(
          `uncaught error while rendering ${nodeResponse.req.url}`,
          error,
        )
      })
    })

    let result = await reader.read()
    while (!result.done) {
      nodeResponse.write(result.value)
      result = await reader.read()
    }
    nodeResponse.end()
  } catch (error) {
    console.error(
      `internal error while streaming response for URL ${nodeResponse.req.url}`,
      { error },
    )
    nodeResponse.end()
  }
}

function convertToNodejsResponseHeaders(
  headers: Headers,
): Record<string, string> {
  const result: Record<string, string> = {}

  for (const [key, value] of headers.entries()) {
    result[key] = value
  }

  return result
}

export async function writeToSocketResponse(
  response: Response,
  socket: Socket,
) {
  const { status, headers, body } = response

  socket.write(`HTTP/1.1 ${status} ${STATUS_CODES[status]}\r\n`)
  for (const [key, value] of headers.entries()) {
    socket.write(`${key}: ${value}\r\n`)
  }

  socket.write("\r\n")

  if (!body) {
    socket.end()
    return
  }

  const reader = body.getReader()

  let result = await reader.read()
  while (!result.done) {
    socket.write(result.value)
    result = await reader.read()
  }

  socket.end()
}

function handleErrorDefault(error: unknown) {
  console.error("error in http request handler", { error })
}
