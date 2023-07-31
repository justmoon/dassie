import { createCliFormatter } from "../../formatters/cli-formatter"

const formatter = createCliFormatter()

process.stdout.write(
  formatter({
    type: "debug",
    namespace: "basic",
    date: Date.now(),
    message: "Debug message",
    parameters: [],
  })
)

process.stdout.write(
  formatter({
    type: "info",
    namespace: "basic",
    date: Date.now(),
    message: "Information message",
    parameters: [123.456, true, "abc", 123n],
  })
)

process.stdout.write(
  formatter({
    type: "warn",
    namespace: "basic",
    date: Date.now(),
    message: "Warning message",
    parameters: [
      {
        foo: "bar",
        baz: 123,
        long: "long long long long long long long long long long long long long long long long long long long long long long long long long long long long long long long long long long long long long long long long long long long long long long long long long long long long long long long long long long long long long long long long long long long long long long long long long long long long long long long long long long long long long long long long long long long long long long long long long long long long long long long long long long long long long long long long long long long long long long long long long long long long long long long long long long long long long long long long long long long long long long long long long long long long long long long long long long long long long long long long long string",
        arr: [1, 2, 3],
        obj: { foo: true },
        longarr: [
          111, 222, 333, 444, 555, 666, 777, 888, 999, 111, 222, 333, 444, 555,
          666, 777, 888, 999, 111, 222, 333, 444, 555, 666, 777, 888, 999,
        ],
      },
    ],
  })
)

process.stdout.write(
  formatter({
    type: "error",
    namespace: "basic",
    date: Date.now(),
    message: "Error message",
    parameters: [{ error: new Error("Something went wrong") }],
  })
)
