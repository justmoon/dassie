import { showComparison } from "./util/compare"

const functions = {
  func: (_foo: number, _bar: string) => {
    // no-op
  },
  generator: function* () {
    // no-op
  },
  asyncfun: async function () {
    // no-op
  },
  named: function myname() {
    // no-op
  },
  anonymous: (
    () => () =>
      void 0
  )(),
}

showComparison(functions, "Functions")
