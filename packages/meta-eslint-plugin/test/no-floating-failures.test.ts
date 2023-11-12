import { rule } from "../src/rules/no-floating-failures"
import { ruleTester } from "./utils/rule-tester"

ruleTester.run("no-floating-failures", rule, {
  valid: [
    {
      name: "handle failure using isFailure",
      code: `
import { Failure, isFailure } from './faiure'

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
import { Failure, isFailure } from './failure'

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
import { Failure, isFailure } from './failure'

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
import { Failure, isFailure } from './failure'

const doSomething = (): Failure | number => 5

const runCode = () => {
  return doSomething()
}
`,
    },
    {
      name: "return failure from arrow function",
      code: `
import { Failure, isFailure } from './failure'

const doSomething = (): Failure | number => 5

const runCode = () => doSomething()
`,
    },
    {
      name: "handle failure using isFailure and access name",
      code: `
import { Failure, isFailure } from './failure'

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
import { Failure, isFailure } from './failure'

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
import { Failure, isFailure } from './failure'

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
import { Failure, isFailure } from './failure'

const doSomething = (): Failure | number => 5

const runCode = () => {
  void doSomething()
}
`,
    },
    {
      name: "throw an error using failure as cause",
      code: `
import { Failure, isFailure } from './failure'

const doSomething = (): Failure | number => 5

const runCode = () => {
  const result = doSomething()

  if (isFailure(result)) {
    throw new Error("failure occurred", { cause: result })
  }
}
`,
    },
    {
      name: "properly handled async failure",
      code: `
import { Failure, isFailure } from './failure'

const doSomethingAsync = async (): Promise<Failure | number> => 5

const runCode = async () => {
  const result = await doSomethingAsync()

  if (isFailure(result)) {
    console.log('failure handled')
  }
}
`,
    },
    {
      name: "returned async failure",
      code: `
import { Failure, isFailure } from './failure'

const doSomethingAsync = async (): Promise<Failure | number> => 5

const runCode = async () => {
  return doSomethingAsync()
}
`,
    },
    {
      name: "returned awaited async failure",
      code: `
import { Failure, isFailure } from './failure'

const doSomethingAsync = async (): Promise<Failure | number> => 5

const runCode = async () => {
  return await doSomethingAsync()
}
`,
    },
    {
      name: "returned assigned async failure",
      code: `
import { Failure } from './failure'
const doSomethingAsync = async (): Promise<Failure | number> => 5

const runCode = async () => {
  const result = await doSomethingAsync()
  return result
}
`,
    },
  ],
  invalid: [
    {
      name: "unhandled failure from variable",
      code: `
import { Failure } from './failure'
declare const fail: Failure

const runCode = () => {
  fail
}
`,
      errors: [
        {
          messageId: "noFloatingFailures",
          line: 6,
        },
      ],
    },
    {
      name: "unhandled failure in union from variable",
      code: `
import { Failure } from './failure'
declare const fail: Failure | number

const runCode = () => {
  fail
}
`,
      errors: [
        {
          messageId: "noFloatingFailures",
          line: 6,
        },
      ],
    },
    {
      name: "unhandled failure intersection from variable",
      code: `
import { Failure } from './failure'
declare const fail: Failure & number

const runCode = () => {
  fail
}
`,
      errors: [
        {
          messageId: "noFloatingFailures",
          line: 6,
        },
      ],
    },
    {
      name: "unhandled failure subclass from variable",
      code: `
import { Failure } from './failure'
class MyFailure extends Failure {
  name = "MyFailure" as const
}
declare const fail: MyFailure

const runCode = () => {
  fail
}
`,
      errors: [
        {
          messageId: "noFloatingFailures",
          line: 9,
        },
      ],
    },
    {
      name: "unhandled failure from function call",
      code: `
import { Failure } from './failure'
declare const doSomething: () => Failure

const runCode = () => {
  doSomething()
}
`,
      errors: [
        {
          messageId: "noFloatingFailures",
          line: 6,
        },
      ],
    },
    {
      name: "unhandled failure in union from function call",
      code: `
import { Failure } from './failure'
declare const doSomething: () => Failure | number

const runCode = () => {
  doSomething()
}
`,
      errors: [
        {
          messageId: "noFloatingFailures",
          line: 6,
        },
      ],
    },
    {
      name: "unhandled failure intersection from function call",
      code: `
import { Failure } from './failure'
declare const doSomething: () => Failure & number

const runCode = () => {
  doSomething()
}
`,
      errors: [
        {
          messageId: "noFloatingFailures",
          line: 6,
        },
      ],
    },
    {
      name: "unhandled failure subclass from function call",
      code: `
import { Failure } from './failure'
class MyFailure extends Failure {
  name = "MyFailure" as const
}
declare const doSomething: () => MyFailure

const runCode = () => {
  doSomething()
}
`,
      errors: [
        {
          messageId: "noFloatingFailures",
          line: 9,
        },
      ],
    },
    {
      name: "should detect unhandled async failure",
      code: `
import { Failure, isFailure } from './failure'
declare const doSomethingAsync: () => Promise<Failure | number>

const runCode = async () => {
  await doSomethingAsync()
}
`,
      errors: [
        {
          messageId: "noFloatingFailures",
          line: 6,
        },
      ],
    },
    {
      name: "unhandled failure in a comma expression",
      code: `
import { Failure, isFailure } from './failure'
declare const fail: Failure

const runCode = () => {
  fail, 123
  123, fail
  123, fail, 123
}
`,
      errors: [
        {
          messageId: "noFloatingFailures",
          line: 6,
        },
        {
          messageId: "noFloatingFailures",
          line: 7,
        },
        {
          messageId: "noFloatingFailures",
          line: 8,
        },
      ],
    },
    {
      name: "unhandled failure returned by optional chaining",
      code: `
import { Failure } from './failure'

const doSomething = (
  obj1: { a?: { b?: { c?: () => Failure } } },
  obj2: { a?: { b?: { c: () => Failure } } },
  obj3: { a?: { b: { c?: () => Failure } } },
  obj4: { a: { b: { c?: () => Failure } } },
  obj5: { a?: () => { b?: { c?: () => Failure } } },
  obj6?: { a: { b: { c?: () => Failure } } },
  callback?: () => Failure,
): void => {
  obj1.a?.b?.c?.();
  obj2.a?.b?.c();
  obj3.a?.b.c?.();
  obj4.a.b.c?.();
  obj5.a?.().b?.c?.();
  obj6?.a.b.c?.();

  callback?.();
};

doSomething();
      `,
      errors: [
        {
          line: 13,
          messageId: "noFloatingFailures",
        },
        {
          line: 14,
          messageId: "noFloatingFailures",
        },
        {
          line: 15,
          messageId: "noFloatingFailures",
        },
        {
          line: 16,
          messageId: "noFloatingFailures",
        },
        {
          line: 17,
          messageId: "noFloatingFailures",
        },
        {
          line: 18,
          messageId: "noFloatingFailures",
        },
        {
          line: 20,
          messageId: "noFloatingFailures",
        },
      ],
    },
    {
      name: "failure returned from iife",
      code: `
import { Failure, isFailure } from './failure'
declare const fail: Failure

function test() {
  (() => fail)();
}
      `,
      errors: [
        {
          line: 6,
          messageId: "noFloatingFailures",
        },
      ],
    },
    {
      name: "inline declared function returning failure",
      code: `
import { Failure, isFailure } from './failure'
declare const fail: Failure

function test() {
  function returnsFailure() { return fail }

  returnsFailure();
}
      `,
      errors: [
        {
          line: 8,
          messageId: "noFloatingFailures",
        },
      ],
    },
    {
      name: "ternary expression returning failure",
      code: `
import { Failure, isFailure } from './failure'
declare const fail: Failure

function test() {
  Math.random() > 0.5 ? fail : null
  Math.random() > 0.5 ? null : fail
}
      `,
      errors: [
        {
          line: 6,
          messageId: "noFloatingFailures",
        },
        {
          line: 7,
          messageId: "noFloatingFailures",
        },
      ],
    },
    {
      name: "unhandled failure from object property access",
      code: `
import { Failure } from './failure'
declare const fail: Failure

async function test() {
  const obj = { foo: fail };
  obj.foo;
}
      `,
      errors: [
        {
          line: 7,
          messageId: "noFloatingFailures",
        },
      ],
    },
    {
      name: "unhandled instantiated failure",
      code: `
import { Failure } from './failure'
class MyFailure extends Failure {
  name = "MyFailure" as const
}
async function test() {
  new MyFailure();
}
      `,
      errors: [
        {
          line: 7,
          messageId: "noFloatingFailures",
        },
      ],
    },
    {
      name: "failure returned from logical or expression containing a voided operand",
      code: `
import { Failure } from './failure'
declare const fail: Failure
declare const condition: boolean

void condition || fail
      `,
      errors: [
        {
          line: 6,
          messageId: "noFloatingFailures",
          suggestions: [
            {
              messageId: "floatingFixVoid",
              output: `
import { Failure } from './failure'
declare const fail: Failure
declare const condition: boolean

void (void condition || fail)
      `,
            },
          ],
        },
      ],
    },
    {
      name: "failure returned from logical or expression",
      code: `
import { Failure } from './failure'
declare const fail: Failure
declare const condition: boolean

condition || fail
fail || condition
      `,
      errors: [
        {
          line: 6,
          messageId: "noFloatingFailures",
          suggestions: [
            {
              messageId: "floatingFixVoid",
              output: `
import { Failure } from './failure'
declare const fail: Failure
declare const condition: boolean

void (condition || fail)
fail || condition
      `,
            },
          ],
        },
        {
          line: 7,
          messageId: "noFloatingFailures",
          suggestions: [
            {
              messageId: "floatingFixVoid",
              output: `
import { Failure } from './failure'
declare const fail: Failure
declare const condition: boolean

condition || fail
void (fail || condition)
      `,
            },
          ],
        },
      ],
    },
    {
      name: "failure returned from logical and expression",
      code: `
import { Failure } from './failure'
declare const fail: Failure
declare const condition: boolean

condition && fail
      `,
      errors: [
        {
          line: 6,
          messageId: "noFloatingFailures",
          suggestions: [
            {
              messageId: "floatingFixVoid",
              output: `
import { Failure } from './failure'
declare const fail: Failure
declare const condition: boolean

void (condition && fail)
      `,
            },
          ],
        },
      ],
    },
    {
      name: "failure returned from nullish coalescing operator",
      code: `
import { Failure } from './failure'
declare const fail: Failure
declare const condition: true | null

condition ?? fail
      `,
      errors: [
        {
          line: 6,
          messageId: "noFloatingFailures",
          suggestions: [
            {
              messageId: "floatingFixVoid",
              output: `
import { Failure } from './failure'
declare const fail: Failure
declare const condition: true | null

void (condition ?? fail)
      `,
            },
          ],
        },
      ],
    },
  ],
})
