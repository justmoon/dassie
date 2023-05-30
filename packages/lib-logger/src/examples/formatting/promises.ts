import { showComparison } from "./util/compare"

const promises = {
  undef: Promise.resolve(),
  bool: Promise.resolve(true),
  null: Promise.resolve(null),
  unresolved: new Promise(() => {
    // no-op
  }),
  rejected: Promise.reject(new Error("Rejected")),
}

showComparison(promises, "Promises")
