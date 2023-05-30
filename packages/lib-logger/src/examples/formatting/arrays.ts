import { showComparison } from "./util/compare"

const arrays = {
  basic: ["Basic value"],
  manyshort: [
    1, 2, 3, 4, 5, 6, 7, 8, 9, 0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 0, 1, 2, 3, 4, 5,
    1, 2, 3, 4, 5,
  ],
  fewlong: [
    "long long long long long",
    "long long long long long",
    "long long long long long",
    "long long long long long",
    "long long long long long",
    "long long long long long",
  ],
  numgrid: [1, 9_999_999, 1, 9_999_999, 1, 9_099_999, 1, 9_999_999],
  mixed: [
    123,
    "long long",
    456,
    "long long",
    789,
    "long long",
    123,
    "long long",
  ],
  sparse: (() => {
    const array = []
    array[6] = true
    return array
  })(),
}

showComparison(arrays, "Arrays")
