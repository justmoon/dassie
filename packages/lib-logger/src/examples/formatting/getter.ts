import { showComparison } from "./util/compare"

const getters = {
  basic: {
    get getset() {
      return "Basic value"
    },
    set getset(_value) {
      // no-op
    },
    get get() {
      return "Getter only"
    },
    set set(_value: string) {
      // no-op
    },
  },
}

showComparison(getters, "Getters and setters")
