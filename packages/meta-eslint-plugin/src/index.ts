import recommended from "./configs/recommended.js"
import mustHandleFailure from "./rules/must-handle-failure.js"

module.exports = {
  configs: {
    recommended,
  },
  rules: {
    "must-handle-failure": mustHandleFailure,
  },
}
