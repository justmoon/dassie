import { createCliFormatter } from "../../formatters/cli-formatter"

const formatter = createCliFormatter()

process.stdout.write(
  formatter({
    type: "debug",
    date: new Date(),
    message: "Debug message",
    parameters: [],
  })
)

process.stdout.write(
  formatter({
    type: "info",
    date: new Date(),
    message: "Information message",
    parameters: [123.456, true, "abc", 123n],
  })
)

process.stdout.write(
  formatter({
    type: "warn",
    date: new Date(),
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
    date: new Date(),
    message: "Error message",
    parameters: [{ error: new Error("Something went wrong") }],
  })
)
