import { showComparison } from "./util/compare"

const keysTestObject = {
  basic: "Basic key",
  0: "Numeric key",
  "complex-key": "Complex key",
  [Symbol("symbol-key")]: "Symbol key",
  [Symbol.for("symbol-for-key")]: "Symbol.for key",
}

showComparison(keysTestObject, "Keys test")
