import { createLibraryConfig } from "../../rollup.common.config.js"

export default createLibraryConfig({
  index: "src/index.ts",
  runner: "src/runner.ts",
})
