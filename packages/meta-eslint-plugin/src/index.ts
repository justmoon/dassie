import recommended from "./configs/recommended.js"
import { rule as noFloatingFailures } from "./rules/no-floating-failures.js"
import { rule as noMisusedFailures } from "./rules/no-misused-failures.js"
import { rule as noTopLevelMutables } from "./rules/no-top-level-mutables.js"
import { rule as noTopLevelSideEffects } from "./rules/no-top-level-side-effects.js"

module.exports = {
  configs: {
    recommended,
  },
  rules: {
    "no-floating-failures": noFloatingFailures,
    "no-misused-failures": noMisusedFailures,
    "no-top-level-mutables": noTopLevelMutables,
    "no-top-level-side-effects": noTopLevelSideEffects,
  },
}
