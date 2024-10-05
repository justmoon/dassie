import path from "node:path"

export const LOCAL_FOLDER = new URL("../../../../local/", import.meta.url)
  .pathname

export const DEV_SERVER_STATE_PATH = path.resolve(LOCAL_FOLDER, "dev/")
