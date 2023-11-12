import recommended from "./configs/recommended.js"
import { rule as noFloatingFailures } from "./rules/no-floating-failures.js"
import { rule as noMisusedFailures } from "./rules/no-misused-failures.js"

module.exports = {
  configs: {
    recommended,
  },
  rules: {
    "no-floating-failures": noFloatingFailures,
    "no-misused-failures": noMisusedFailures,
  },
}
