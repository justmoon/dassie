import { createLibraryConfig } from "../../common/config/rollup.js"

export default createLibraryConfig({
  server: "./server.ts",
  client: "./client.ts",
})
