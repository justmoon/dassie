import { rule } from "../src/rules/no-misused-failures"
import { ruleTester } from "./utils/rule-tester"

ruleTester.run("no-misused-failures", rule, {
  valid: [
    {
      name: "boolean unary not with non-Failure value",
      code: `
!true
`,
    },
    {
      name: "if statement with non-Failure value",
      code: `
if (true) {}
`,
    },
    {
      name: "if-else statement with non-Failure value",
      code: `
if (true) {
} else if (false) {
} else {
}
`,
    },
    {
      name: "while loop with non-Failure value",
      code: `
while (true) {}
`,
    },
    {
      name: "do-while loop with non-Failure value",
      code: `
do {} while (true)
`,
    },
    {
      name: "for loop with non-Failure value",
      code: `
for (;;) {}
for (let i; i < 10; i++) {}
`,
    },
    {
      name: "logical expression with non-Failure value",
      code: `
true && false
`,
    },
    {
      name: "ternary expression with non-Failure value",
      code: `
true ? false : true
`,
    },
    {
      name: "if statement with Failure when checksConditionals is false",
      code: `
import { Failure } from './failure'
declare const fail: Failure

if (fail) {}
      `,
      options: [{ checksConditionals: false }],
    },
    {
      name: "if-else statement with Failure when checksConditionals is false",
      code: `
import { Failure } from './failure'
declare const fail: Failure
declare const fail2: Failure

if (fail) {
} else if (fail2) {
} else {
}
      `,
      options: [{ checksConditionals: false }],
    },
    {
      name: "for loop with Failure when checksConditionals is false",
      code: `
import { Failure } from './failure'
declare const fail: Failure

for (let i; fail; i++) {}
`,
      options: [{ checksConditionals: false }],
    },
    {
      name: "do-while loop with Failure when checksConditionals is false",
      code: `
import { Failure } from './failure'
declare const fail: Failure

do {} while (fail);
`,
      options: [{ checksConditionals: false }],
    },
    {
      name: "while loop with Failure when checksConditionals is false",
      code: `
import { Failure } from './failure'
declare const fail: Failure

while (fail) {}
`,
      options: [{ checksConditionals: false }],
    },
    {
      name: "ternary expression with Failure when checksConditionals is false",
      code: `
import { Failure } from './failure'
declare const fail: Failure

fail ? 123 : 456;
`,
      options: [{ checksConditionals: false }],
    },
    {
      name: "negated if statement with no Failure",
      code: `
if (!true) {
}
`,
    },
    {
      name: "negated if statement with Failure when checksConditionals is false",
      code: `
import { Failure } from './failure'
declare const fail: Failure

if (!fail) {
}
      `,
      options: [{ checksConditionals: false }],
    },
    {
      name: "failure inside if statement using isFailure",
      code: `
import { Failure, isFailure } from './failure'
declare const fail: Failure

if (isFailure(fail)) {
}
`,
    },
    {
      name: "logical expression with Failure when checksConditionals is false",
      code: `
import { Failure, isFailure } from './failure'
declare const fail: Failure

fail || false;
`,
      options: [{ checksConditionals: false }],
    },
    {
      name: "nested logical expression with Failure when checksConditionals is false",
      code: `
import { Failure, isFailure } from './failure'
declare const fail: Failure

(true && fail) || false;`,
      options: [{ checksConditionals: false }],
    },
    {
      name: "nested logical expression returning Failure",
      code: `
import { Failure, isFailure } from './failure'
declare const fail: Failure

false || (true && fail);`,
    },
    {
      name: "nested logical expression returning Failure 2",
      code: `
import { Failure, isFailure } from './failure'
declare const fail: Failure

(true && fail) || false;
`,
    },
    {
      name: "check a Failure using isFailure",
      code: `
import { Failure, isFailure } from './failure'
declare const fail: Failure

if (isFailure(fail)) {
}
`,
    },
    {
      name: "extract a Failure from a union using if statement",
      code: `
import { Failure } from './failure'
declare const fail: Failure

const runCode = () => {
  const mixed: Failure | undefined = Math.random() > 0.5 ? fail : undefined
  if (mixed) {
    return mixed
  }
}
`,
    },
    {
      name: "ignore something that is not quite a Failure",
      code: `
interface NotQuiteFailure {
  ["some.other.key"]: true
  readonly name: string
}
const value: NotQuiteFailure = { ["some.other.key"]: true, name: "foo" }
if (value) {
}
`,
    },
    {
      name: "simple foreach",
      code: "[1, 2, 3].forEach(val => {});",
    },
    {
      name: "simple foreach returning failure when checksVoidReturn is false",
      code: `
import { Failure } from './failure'
declare const fail: Failure

[1, 2, 3].forEach(val => fail)
`,
      options: [{ checksVoidReturn: false }],
    },
    {
      name: "allow mapping over values, followed by findFailure",
      code: `
import { Failure, findFailure } from './failure'
declare const fail: Failure

findFailure(
  ['abc', 'def'].map(val => fail),
)
`,
    },
    {
      name: "allow callbacks returning failures when they are explicitly expected",
      code: `
import { Failure } from './failure'
declare const fail: Failure

const fn: (arg: () => Failure | void) => void = () => {}
fn(() => fail)
`,
    },
    {
      name: "allow optional chain on a failure-returning function",
      code: `
import { Failure } from './failure'

declare const returnsFailure: (() => Failure) | null
if (returnsFailure?.()) {
}
`,
    },
    {
      name: "allow optional chain on a failure-returning method",
      code: `
import { Failure } from './failure'

declare const returnsFailure: { call: () => Failure } | null
if (returnsFailure?.call()) {
}
`,
    },
    {
      name: "allow nullish coalescing operator on a failure value",
      code: `
import { Failure } from './failure'
declare const fail: Failure
      
fail ?? false
`,
    },
    {
      name: "allow nullish coalescing operator on a failure value when default is also a failure",
      code: `
import { Failure } from './failure'
declare const fail: Failure
      
function test(a: Failure | undefined) {
  const foo = a ?? fail
}
`,
    },
    {
      name: "allow nullish coalescing operator on a failure value when default is a boolean",
      code: `
import { Failure } from './failure'
      
function test(p: Failure | undefined, bool: boolean) {
  if (p ?? bool) {
  }
}
`,
    },
    {
      name: "assign a failure-returning function",
      code: `
import { Failure } from './failure'
declare const fail: Failure

let f
f = () => fail
`,
    },
    {
      name: "failure-returning function assignments",
      code: `
import { Failure } from './failure'
declare const fail: Failure

let f: () => Failure
f = () => fail
const g = () => fail
const h: () => Failure | number = () => Math.random() > 0.5 ? fail : 123
`,
    },
    {
      name: "object with a failure-returning method",
      code: `
import { Failure } from './failure'
declare const fail: Failure

const obj = {
  f: () => fail,
}
`,
    },
    {
      name: "object with a pre-existing failure-returning method",
      code: `
import { Failure } from './failure'
declare const fail: Failure

const f = () => fail
const obj = {
  f,
}
`,
    },
    {
      name: "object with inline failure-returning method",
      code: `
import { Failure } from './failure'
declare const fail: Failure

const obj = {
  f() {
    return fail
  },
}
`,
    },
    {
      name: "assigning to an object type containing failure-returning methods",
      code: `
import { Failure } from './failure'
declare const fail: Failure

type O = { f: () => Failure; g: () => Failure };
const g = () => fail;
const obj: O = {
  f: () => fail,
  g,
};
`,
    },
    {
      name: "failure-returning inline method with a dynamic name",
      code: `
import { Failure } from './failure'
declare const fail: Failure

type O = { f: () => Failure };
const name = 'f';
const obj: O = {
  [name]() {
    return fail;
  },
};
`,
    },
    {
      name: "method returning a non-failure",
      code: `
const obj: number = {
  g() {
    return 10;
  },
};
`,
    },
    {
      name: "function returning a failure-returning function",
      code: `
import { Failure } from './failure'
declare const fail: Failure

function f() {
  return () => fail;
}
`,
    },
    {
      name: "function returning a non-failure",
      code: `
function g() {
  return 25;
}
`,
    },
    {
      name: "passing a failure-returning callback in JSX",
      code: `
import { Failure } from './failure'
declare const fail: Failure

type O = {
  bool: boolean
  func: () => Failure
}
const Component = (obj: O) => null;
<Component bool func={() => fail} />;
`,
      parserOptions: {
        ecmaFeatures: {
          jsx: true,
        },
      },
    },
    {
      name: "passing a failure-returning callback in JSX when Component is any",
      code: `
import { Failure } from './failure'
declare const fail: Failure

const Component: any = () => null;
<Component func={() => fail} />;
`,
      parserOptions: {
        ecmaFeatures: {
          jsx: true,
        },
      },
    },
    {
      name: "passing a failure-returning callback to an overloaded function using the first version",
      code: `
import { Failure } from './failure'
declare const fail: Failure

interface Overloaded {
  (name: string, callback: () => Failure): void;
  (name: string, callback: () => void): void;
}

declare const doSomething: Overloaded;

doSomething('', () => fail);
`,
    },
    {
      name: "passing a failure-returning callback to an overloaded function using the second version",
      code: `
import { Failure } from './failure'
declare const fail: Failure

interface Overloaded {
  (name: string, callback: () => void): void;
  (name: string, callback: () => Failure): void;
}

declare const doSomething: Overloaded;

doSomething('', () => fail);
`,
    },
    {
      name: "passing a failure-returning callback to a function overloaded via multiple interfaces using the first version",
      code: `
import { Failure } from './failure'
declare const fail: Failure

interface Overloaded {
  (name: string, callback: () => Failure): void;
}
interface Overloaded {
  (name: string, callback: () => void): void;
}

declare const doSomething: Overloaded;

doSomething('', () => fail);
`,
    },
    {
      name: "passing a failure-returning callback to a function overloaded via multiple interfaces using the second version",
      code: `
import { Failure } from './failure'
declare const fail: Failure

interface Overloaded {
  (name: string, callback: () => void): void;
}
interface Overloaded {
  (name: string, callback: () => Failure): void;
}

declare const doSomething: Overloaded;

doSomething('', () => fail);
`,
    },
    {
      name: "passing a failure-returning callback to a JSX property with a union of function types",
      code: `
import { Failure } from './failure'
declare const fail: Failure

interface Props {
  onEvent: (() => void) | (() => Failure);
}

declare function Component(props: Props): any;

const _ = <Component onEvent={() => fail} />;
`,
      parserOptions: {
        ecmaFeatures: {
          jsx: true,
        },
      },
    },
    {
      name: "spread operator on a failure when the checksSpreads option is false",
      code: `
import { Failure } from './failure'
declare const fail: Failure

console.log({ ...fail });
`,
      options: [{ checksSpreads: false }],
    },
    {
      name: "spread operator on a failure returned from a function when the checksSpreads option is false",
      code: `
import { Failure } from './failure'
declare const fail: Failure

const getData = () => fail;

console.log({
  someData: 42,
  ...getData(),
});
`,
      options: [{ checksSpreads: false }],
    },
    {
      name: "spread operator on a failure resulting from a conditional when the checksSpreads option is false",
      code: `
import { Failure } from './failure'
declare const fail: Failure

declare const condition: boolean;

console.log({ ...(condition && fail) });
console.log({ ...(condition || fail) });
console.log({ ...(condition ? {} : fail) });
console.log({ ...(condition ? fail : {}) });
`,
      options: [{ checksSpreads: false }],
    },
    {
      name: "spread operator on a failure in an array when the checksSpreads option is false",
      code: `
import { Failure } from './failure'
declare const fail: Failure

// This is invalid Typescript, but it shouldn't trigger this linter specifically
console.log([...fail]);
`,
      options: [{ checksSpreads: false }],
    },
    {
      name: "failure-returning functions passed to arguments defined as spread any",
      code: `
import { Failure } from './failure'
declare const fail: Failure

function spreadAny(..._args: any): void {}

spreadAny(
  true,
  () => fail,
  () => fail,
);
`,
    },
    {
      name: "failure-returning functions passed to arguments defined as spreading an array of any",
      code: `
import { Failure } from './failure'
declare const fail: Failure

function spreadArrayAny(..._args: Array<any>): void {}

spreadArrayAny(
  true,
  () => fail,
  () => fail,
);
`,
    },
    {
      name: "failure-returning functions passed to arguments defined as spreading an array of unknown",
      code: `
import { Failure } from './failure'
declare const fail: Failure

function spreadArrayUnknown(..._args: Array<unknown>): void {}

spreadArrayUnknown(() => Promise.resolve(true), 1, 2);

function spreadArrayFuncPromise(
  ..._args: Array<() => Promise<undefined>>
): void {}

spreadArrayFuncPromise(
  () => fail,
  () => fail,
);
`,
    },
    {
      name: "constructor accepting non-failure-returning callbacks",
      code: `
class TakeCallbacks {
  constructor(...callbacks: Array<() => void>) {}
}

new TakeCallbacks;
new TakeCallbacks();
new TakeCallbacks(
  () => 1,
  () => true,
);
    `,
    },
    {
      name: "functions accepting spread tuple arguments",
      code: `
function restTuple(...args: []): void;
function restTuple(...args: [string]): void;
function restTuple(..._args: string[]): void {}

restTuple();
restTuple('Hello');
    `,
    },
    {
      name: "assigning a non-failure-returning function to a Record",
      code: `
      let value: Record<string, () => void>;
      value.sync = () => {};
    `,
    },
    {
      name: "a function that returns a Record of non-failure-returning functions",
      code: `
      type ReturnsRecord = () => Record<string, () => void>;

      const test: ReturnsRecord = () => {
        return { sync: () => {} };
      };
    `,
    },
    {
      name: "a function that returns a Record of non-failure-returning functions using a pre-defined function",
      code: `
      type ReturnsRecord = () => Record<string, () => void>;

      function sync() {}

      const test: ReturnsRecord = () => {
        return { sync };
      };
    `,
    },
  ],
  invalid: [
    {
      name: "not allowed to use failure in a boolean unary not expression as an ExpressionStatement",
      code: `
import { Failure } from './failure'
declare const fail: Failure

!fail
`,
      errors: [
        {
          messageId: "conditional",
          line: 5,
        },
      ],
    },
    {
      name: "not allowed to use failure in an if statement",
      code: `
import { Failure } from './failure'
declare const fail: Failure

if (fail) {}
`,
      errors: [
        {
          messageId: "conditional",
          line: 5,
        },
      ],
    },
    {
      name: "not allowed to use failure in an if-else statement",
      code: `
import { Failure } from './failure'
declare const fail: Failure
declare const fail2: Failure

if (fail) {
} else if (fail2) {
} else {
}
`,
      errors: [
        {
          messageId: "conditional",
          line: 6,
        },
        {
          messageId: "conditional",
          line: 7,
        },
      ],
    },
    {
      name: "not allowed to use failure in a while loop",
      code: `
import { Failure } from './failure'
declare const fail: Failure

while (fail) {}
`,
      errors: [
        {
          messageId: "conditional",
          line: 5,
        },
      ],
    },
    {
      name: "not allowed to use failure in a do-while loop",
      code: `
import { Failure } from './failure'
declare const fail: Failure

do {} while (fail)
`,
      errors: [
        {
          messageId: "conditional",
          line: 5,
        },
      ],
    },
    {
      name: "not allowed to use failure in a for loop",
      code: `
import { Failure } from './failure'
declare const fail: Failure

for (;fail;) {}
`,
      errors: [
        {
          messageId: "conditional",
          line: 5,
        },
      ],
    },
    {
      name: "not allowed to use failure in a logical expression",
      code: `
import { Failure } from './failure'
declare const fail: Failure

fail && true
`,
      errors: [
        {
          messageId: "conditional",
          line: 5,
        },
      ],
    },
    {
      name: "not allowed to use failure in a ternary expression",
      code: `
import { Failure } from './failure'
declare const fail: Failure

fail ? true : false
`,
      errors: [
        {
          messageId: "conditional",
          line: 5,
        },
      ],
    },
    {
      name: "negated if statement with Failure",
      code: `
import { Failure } from './failure'
declare const fail: Failure

if (!fail) {
}
`,
      errors: [
        {
          messageId: "conditional",
          line: 5,
        },
      ],
    },
    {
      name: "failure-returning function as forEach callback",
      code: `
import { Failure } from './failure'
declare const fail: Failure

;[1, 2].forEach(val => fail)
      `,
      errors: [
        {
          line: 5,
          messageId: "voidReturnArgument",
        },
      ],
    },
  ],
})
