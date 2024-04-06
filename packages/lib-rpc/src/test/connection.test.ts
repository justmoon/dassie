import { describe, vi } from "vitest"
import { z } from "zod"

import { createSubscription } from "../client"
import { createRoute } from "../server/router/route"
import { createRouter } from "../server/router/router"
import { createServerClientPair } from "./utils/create-server-client-pair"

describe("RPC Connection", (test) => {
  test("should be able to handle requests", async ({ expect }) => {
    const minimalRouter = createRouter({
      ping: createRoute().query(() => "pong" as const),
    })

    const connection = createServerClientPair(minimalRouter, {})

    const result = await connection.rpc.ping.query()

    expect(result).toBe("pong")
  })

  test("should provide context to rpc handlers", async ({ expect }) => {
    const routerWithContext = createRouter({
      getServerName: createRoute()
        .context<{ name: string }>()
        .query(({ context: { name } }) => name),
    })

    const context = { name: "server-name-in-context" }

    const connection = createServerClientPair(routerWithContext, context)

    const result = await connection.rpc.getServerName.query()

    expect(result).toBe("server-name-in-context")
  })

  test("should run middleware", async ({ expect }) => {
    const middlewareSpy = vi.fn()
    const routerWithMiddleware = createRouter({
      ping: createRoute()
        .use(middlewareSpy)
        .query(() => "pong" as const),
    })

    const connection = createServerClientPair(routerWithMiddleware, {})

    const result = await connection.rpc.ping.query()

    expect(result).toBe("pong")
    expect(middlewareSpy).toHaveBeenCalledOnce()
  })

  test("should provide context to middleware", async ({ expect }) => {
    const middlewareSpy = vi.fn()
    const routerWithMiddleware = createRouter({
      ping: createRoute()
        .context<{ name: string }>()
        .use(middlewareSpy)
        .query(() => "pong" as const),
    })

    const context = { name: "server-name-for-middleware" }

    const connection = createServerClientPair(routerWithMiddleware, context)

    const result = await connection.rpc.ping.query()

    expect(result).toBe("pong")
    expect(middlewareSpy).toHaveBeenCalledOnce()
    expect(middlewareSpy).toHaveBeenCalledWith({
      context,
    })
  })

  test("should be able to pass a parameter to a query", async ({ expect }) => {
    const routerWithParameter = createRouter({
      echo: createRoute()
        .input(z.literal("parameter-example"))
        .query(({ input }) => input),
    })

    const connection = createServerClientPair(routerWithParameter, {})

    const result = await connection.rpc.echo.query("parameter-example")

    expect(result).toBe("parameter-example")
  })

  test("should be able to subscribe", async ({ expect }) => {
    const routerWithSubscription = createRouter({
      countWithPrefix: createRoute()
        .input(z.string())
        .subscription(({ input }) => {
          let count = 0
          return createSubscription<string>((onData) => {
            for (let index = 0; index < 5; index++) {
              onData(`${input}-${count++}`)
            }
            return () => {}
          })
        }),
    })

    let done: () => void
    const promise = new Promise<void>((resolve) => (done = resolve))

    const connection = createServerClientPair(routerWithSubscription, {})
    const subscription = await connection.rpc.countWithPrefix.subscribe("super")

    const subscriber = vi.fn((data: string) => {
      if (data === "super-4") done()
    })

    const dispose = subscription((data) => {
      subscriber(data)
    })

    await promise

    expect(subscriber.mock.calls).toMatchInlineSnapshot(`
      [
        [
          "super-0",
        ],
        [
          "super-1",
        ],
        [
          "super-2",
        ],
        [
          "super-3",
        ],
        [
          "super-4",
        ],
      ]
    `)

    dispose()
  })
})
