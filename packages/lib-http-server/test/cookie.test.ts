import { describe, test } from "vitest"

import { cookie } from "../src"
import { makeHeadersContext } from "./fixtures/make-headers-context"

describe("cookie", () => {
  test("should handle a request with no cookie header", ({ expect }) => {
    const result = cookie(makeHeadersContext({}))
    expect(result).toStrictEqual({ cookies: {} })
  })

  test("should handle a request with an empty cookie header", ({ expect }) => {
    const result = cookie(makeHeadersContext({ cookie: "" }))
    expect(result).toStrictEqual({ cookies: {} })
  })

  test("should handle a request with a cookie header with one cookie", ({
    expect,
  }) => {
    const result = cookie(makeHeadersContext({ cookie: "a=b" }))
    expect(result).toStrictEqual({ cookies: { a: "b" } })
  })

  test("should handle a request with a cookie header with two cookies", ({
    expect,
  }) => {
    const result = cookie(makeHeadersContext({ cookie: "a=b; c=d" }))
    expect(result).toStrictEqual({ cookies: { a: "b", c: "d" } })
  })

  test("should ignore spaces", ({ expect }) => {
    const result = cookie(
      makeHeadersContext({ cookie: " a    = b ; c =    d  " }),
    )
    expect(result).toStrictEqual({ cookies: { a: "b", c: "d" } })
  })

  test("should handle a request with a cookie header with a quoted cookie", ({
    expect,
  }) => {
    const result = cookie(makeHeadersContext({ cookie: 'a="b"' }))
    expect(result).toStrictEqual({ cookies: { a: "b" } })
  })

  test("should use the first value if a cookie is set twice", ({ expect }) => {
    const result = cookie(makeHeadersContext({ cookie: "a=b; a=c" }))
    expect(result).toStrictEqual({ cookies: { a: "b" } })
  })

  test("should allow a cookie with an empty value", ({ expect }) => {
    const result = cookie(makeHeadersContext({ cookie: "a=; b=a" }))
    expect(result).toStrictEqual({ cookies: { a: "", b: "a" } })
  })

  test("should ignore cookies containing whitespace in the token", ({
    expect,
  }) => {
    const result = cookie(makeHeadersContext({ cookie: "a b=c" }))
    expect(result).toStrictEqual({ cookies: {} })
  })

  test("should ignore cookies containing whitespace in the value", ({
    expect,
  }) => {
    const result = cookie(makeHeadersContext({ cookie: "a=b c" }))
    expect(result).toStrictEqual({ cookies: {} })
  })

  test("should ignore cookies containing whitespace in the token", ({
    expect,
  }) => {
    const result = cookie(makeHeadersContext({ cookie: "a b=c" }))
    expect(result).toStrictEqual({ cookies: {} })
  })

  test("should ignore cookies containing whitespace in the value", ({
    expect,
  }) => {
    const result = cookie(makeHeadersContext({ cookie: "a=b c" }))
    expect(result).toStrictEqual({ cookies: {} })
  })

  test("should ignore cookies containing invalid characters in the token", ({
    expect,
  }) => {
    const result = cookie(makeHeadersContext({ cookie: "a,b=c" }))
    expect(result).toStrictEqual({ cookies: {} })
  })

  test("should ignore cookies containing invalid characters in the value", ({
    expect,
  }) => {
    const result = cookie(makeHeadersContext({ cookie: "a=b,c" }))
    expect(result).toStrictEqual({ cookies: {} })
  })
})
