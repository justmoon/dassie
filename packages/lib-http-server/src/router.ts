import type { Request, Response } from "express"
import { Router } from "express"
import { Simplify } from "type-fest"
import type { AnyZodObject, infer as inferZodType } from "zod"

import { IncomingMessage } from "node:http"

import { Failure, isFailure } from "@dassie/lib-type-utils"

import { BadRequestFailure } from "./failures/bad-request-failure"
import { HttpRequestHandler, createHandler } from "./handler"
import {
  parseBodyBuffer,
  parseBodyUint8Array,
  parseBodyUtf8,
  parseBodyZod,
  parseJson,
} from "./parse-body"
import { HttpFailure } from "./types/http-failure"
import { RouteParameters } from "./types/route-parameters"

export const HTTP_METHODS = [
  "get",
  "post",
  "put",
  "delete",
  "patch",
  "head",
  "options",
] as const

export type HttpMethod = (typeof HTTP_METHODS)[number]

export const BODY_PARSERS = {
  none: async () => {},
  json: parseJson,
  uint8Array: parseBodyUint8Array,
  utf8: parseBodyUtf8,
  buffer: parseBodyBuffer,
} as const

export type BodyParser = keyof typeof BODY_PARSERS

export type RestApiBuilder = {
  [method in HttpMethod]: () => ApiRouteBuilder
}

export type ApiRouteParameters = object

export type ApiHandler<TParameters extends ApiRouteParameters> =
  HttpRequestHandler<Simplify<TParameters>>

export type InferBodyType<TBodyParser extends BodyParser> = Exclude<
  Awaited<ReturnType<(typeof BODY_PARSERS)[TBodyParser]>>,
  Failure
>

export type ApiRouteBuilder<
  TParameters extends ApiRouteParameters = ApiRouteParameters,
> = {
  /**
   * Provide a JSON validator for the request body.
   */
  path: <TPath extends string>(
    path: TPath,
  ) => ApiRouteBuilder<
    // Only add a parameters type to the object if there are actual parameters
    object extends RouteParameters<TPath>
      ? TParameters & {
          params: RouteParameters<TPath>
        }
      : TParameters
  >

  /**
   * Set HTTP method.
   */
  method: (method: HttpMethod) => ApiRouteBuilder<TParameters>

  /**
   * Retrieve the request body and put it into the body property on the request object.
   */
  bodyParser: <TBodyParser extends BodyParser>(
    type: TBodyParser,
  ) => ApiRouteBuilder<
    TParameters & {
      body: InferBodyType<TBodyParser>
    }
  >

  /**
   * Provide a JSON validator for the request body.
   *
   * @remarks
   *
   * This will automatically set the body parser to `json`. If you change the
   * body parser after calling this method, the schema will be disabled.
   */
  bodySchema: <TBodySchema extends AnyZodObject>(
    schema: TBodySchema,
  ) => ApiRouteBuilder<
    TParameters & {
      body: inferZodType<TBodySchema>
    }
  >

  /**
   * Provide a validator for the query string record.
   */
  querySchema: <TQuerySchema extends AnyZodObject>(
    schema: TQuerySchema,
  ) => ApiRouteBuilder<
    TParameters & {
      query: inferZodType<TQuerySchema>
    }
  >

  /**
   * Provide a function which will be run against each request before the
   * handler and which may return a failure to be returned to the client.
   */
  assert: (
    assertion: (request: IncomingMessage) => HttpFailure | void,
  ) => ApiRouteBuilder<TParameters>

  /**
   * Enable CORS for this route.
   */
  cors: () => ApiRouteBuilder<TParameters>

  /**
   * Provide the handler for the API route.
   */
  handler: <THandler extends ApiHandler<TParameters>>(
    handler: THandler,
  ) => ApiRouteInstance
} & {
  [K in HttpMethod]: () => ApiRouteBuilder<TParameters>
}

export interface ApiRouteInstance {
  dispose: () => void
}

interface RouteBuilderState {
  readonly path: string | undefined
  readonly method: HttpMethod | undefined
  readonly bodyParser:
    | ((
        request: IncomingMessage,
        // eslint-disable-next-line @typescript-eslint/no-redundant-type-constituents
      ) => Promise<HttpFailure | unknown>)
    | undefined
  readonly queryValidator: AnyZodObject | undefined
  readonly cors: boolean
  readonly userHandler: ApiHandler<ApiRouteParameters> | undefined
  readonly assertions: Array<(request: IncomingMessage) => HttpFailure | void>
}

const initialRouteBuilderState: RouteBuilderState = {
  path: undefined,
  method: undefined,
  bodyParser: undefined,
  queryValidator: undefined,
  cors: false,
  userHandler: undefined,
  assertions: [],
}

const createRouteHandler =
  (state: RouteBuilderState) =>
  async (request: Request, response: Response) => {
    if (!state.userHandler) {
      throw new Error("No handler provided for API route")
    }

    if (state.bodyParser) {
      const result = await state.bodyParser(request)
      if (isFailure(result)) return result

      request.body = result
    }

    try {
      state.queryValidator?.parse(request.query)
    } catch (error) {
      console.warn("invalid api request query string", {
        path: state.path,
        error,
      })
      return new BadRequestFailure("Invalid API request query string")
    }

    if (state.cors) {
      response.setHeader("Access-Control-Allow-Origin", "*")
    }

    return state.userHandler(request, response)
  }

export const createRouter = () => {
  const router = Router()

  const createBuilder = (state: RouteBuilderState) => {
    const routeDescriptor: ApiRouteBuilder<ApiRouteParameters> = {
      path: (value) => {
        return createBuilder({
          ...state,
          path: value,
        })
      },
      method: (method) => {
        return createBuilder({
          ...state,
          method,
        })
      },
      ...(Object.fromEntries(
        HTTP_METHODS.map(
          (method) =>
            [
              method,
              () =>
                createBuilder({
                  ...state,
                  method,
                }),
            ] as const,
        ),
      ) as RestApiBuilder),
      bodyParser: (type: BodyParser) => {
        return createBuilder({
          ...state,
          bodyParser: BODY_PARSERS[type],
        })
      },
      bodySchema: (schema) => {
        return createBuilder({
          ...state,
          bodyParser: parseBodyZod(schema),
        })
      },
      querySchema: (schema) => {
        return createBuilder({
          ...state,
          queryValidator: schema,
        })
      },
      cors: () => {
        return createBuilder({
          ...state,
          cors: true,
        })
      },
      assert: (assertion) => {
        return createBuilder({
          ...state,
          assertions: [...state.assertions, assertion],
        })
      },
      handler: (handler) => {
        const { path, method } = state
        if (!path) {
          throw new Error("No path provided for route")
        }
        if (!method) {
          throw new Error("No method provided for route")
        }

        const finalState = {
          ...state,
          userHandler: handler,
        }

        const routeHandler = createRouteHandler(finalState)

        router[method](path, createHandler(routeHandler))

        return {
          dispose: () => {
            const routeIndex = router.stack.findIndex(
              (layer: {
                route?: {
                  path: string
                  stack: { handle: HttpRequestHandler }[]
                }
              }) => {
                return (
                  // path is always a string here because we check for it above
                  // and while it can be changed later, it can only be changed
                  // to another string
                  layer.route?.path === path &&
                  layer.route.stack.findIndex(
                    (sublayer) => sublayer.handle === routeHandler,
                  ) !== -1
                )
              },
            )
            if (routeIndex !== -1) router.stack.splice(routeIndex, 1)
          },
        }
      },
    }

    return routeDescriptor
  }

  return {
    ...createBuilder(initialRouteBuilderState),
    middleware: router,
  }
}
