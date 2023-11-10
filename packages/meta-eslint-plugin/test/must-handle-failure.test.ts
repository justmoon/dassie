import { MessageIds } from "../src/message-ids"
import rule from "../src/rules/must-handle-failure"
import { ruleTester } from "./utils/rule-tester"

const COMMON_HEADER = `
const FAILURE_UNIQUE_KEY = "dassie.failure"
class Failure {
  [FAILURE_UNIQUE_KEY]: true
  name: string
}
export const isFailure = <T>(value: T): value is Extract<T, Failure> => {
  return Boolean(
    (value as null | { [FAILURE_UNIQUE_KEY]?: true })?.[FAILURE_UNIQUE_KEY],
  )
}
`

ruleTester.run<MessageIds, []>("must-handle-failure", rule, {
  valid: [
    {
      name: "handle failure using isFailure",
      code: `
${COMMON_HEADER}

const doSomething = (): Failure | number => 5

const runCode = () => {
  const result = doSomething()

  if (isFailure(result)) {
    console.log('failure handled')
    return
  }

  console.log('done')
}
`,
    },
    {
      name: "handle failure using isFailure and access result",
      code: `
${COMMON_HEADER}

const doSomething = (): Failure | number => 5

const runCode = () => {
  const result = doSomething()

  if (isFailure(result)) {
    console.log('failure handled')
    return
  }

  console.log('the result is', result)
}
`,
    },
    {
      name: "return failure using isFailure",
      code: `
${COMMON_HEADER}

const doSomething = (): Failure | number => 5

const runCode = () => {
  const result = doSomething()

  if (isFailure(result)) {
    return result
  }

  return 5 + result
}
`,
    },
    {
      name: "return failure directly",
      code: `
${COMMON_HEADER}

const doSomething = (): Failure | number => 5

const runCode = () => {
  return doSomething()
}
`,
    },
    {
      name: "return failure from arrow function",
      code: `
${COMMON_HEADER}

const doSomething = (): Failure | number => 5

const runCode = () => doSomething()
`,
    },
    {
      name: "handle failure using isFailure and access name",
      code: `
${COMMON_HEADER}

const doSomething = (): Failure | number => 5

const runCode = () => {
  const result = doSomething()

  if (isFailure(result)) {
    console.log('failure handled', failure.name)
    return
  }

  console.log('the result is', result)
}
`,
    },
    {
      name: "handle one of several failures using isFailure",
      code: `
${COMMON_HEADER}

class OneFailure extends Failure {}
class TwoFailure extends Failure {}

const doSomething = (): OneFailure | TwoFailure | number => 5

const runCode = () => {
  const result = doSomething()

  if (isFailure(result)) {
    console.log('failure handled')
  }

  return result
}
`,
    },
    {
      name: "handle one of several failures using isFailure and a switch statement",
      code: `
${COMMON_HEADER}

class OneFailure extends Failure {
  name = "OneFailure" as const
}
class TwoFailure extends Failure {
  name = "TwoFailure" as const
}

const doSomething = (): OneFailure | TwoFailure | number => 5

const runCode = () => {
  const result = doSomething()

  if (isFailure(result)) {
    switch (result.name) {
      case "OneFailure": {
        console.log('failure handled')
        break
      }
      case "TwoFailure": {
        console.log('failure handled')
        break
      }
    }
  } 

  return result
}
`,
    },
    {
      name: "explicitly ignore failure using void",
      code: `
${COMMON_HEADER}

const doSomething = (): Failure | number => 5

const runCode = () => {
  void doSomething()
}
`,
    },
  ],
  invalid: [
    {
      name: "basic unhandled failure",
      code: `
${COMMON_HEADER}

const doSomething = (): Failure | number => 5

const runCode = () => {
  doSomething()
}
`,
      errors: [
        {
          messageId: "mustHandleFailure",
        },
      ],
    },
    {
      name: "unhandled failure where value is passed to a function other than isFailure",
      code: `
${COMMON_HEADER}

const doSomething = (): Failure | number => 5

const isNumber = (x: unknown): x is number => typeof x === "number"
const runCode = () => {
  const result = doSomething()

  if (isNumber(result)) {
    console.log("result is number")
  }
}
`,
      errors: [
        {
          messageId: "mustHandleFailure",
        },
      ],
    },
    {
      name: "unary operators other than void do not suppress the error",
      code: `
${COMMON_HEADER}

const doSomething = (): Failure | number => 5

const runCode = () => {
  !doSomething()
}
`,
      errors: [
        {
          messageId: "mustHandleFailure",
        },
      ],
    },
  ],
})
