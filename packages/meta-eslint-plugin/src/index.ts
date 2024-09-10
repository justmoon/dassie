import recommended from "./configs/recommended.js"
import { rule as noFloatingFailures } from "./rules/no-floating-failures.js"
import { rule as noMisusedFailures } from "./rules/no-misused-failures.js"
import { rule as noTopLevelMutables } from "./rules/no-top-level-mutables.js"
import { rule as noTopLevelSideEffects } from "./rules/no-top-level-side-effects.js"

const plugin = {
  rules: {
    "no-floating-failures": noFloatingFailures,
    "no-misused-failures": noMisusedFailures,
    "no-top-level-mutables": noTopLevelMutables,
    "no-top-level-side-effects": noTopLevelSideEffects,
  },
}

// eslint-disable-next-line unicorn/prefer-module
module.exports = {
  ...plugin,
  configs: {
    recommended: { plugins: { "@dassie": plugin }, ...recommended },
  },
}
