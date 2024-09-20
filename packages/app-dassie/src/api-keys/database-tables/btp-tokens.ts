import { column, table } from "@dassie/lib-sqlite"

import type { BtpToken } from "../types/btp-token"

export const btpTokensTable = table({
  name: "btp_tokens",
  columns: {
    token: column().type("TEXT").typescriptType<BtpToken>().primaryKey(),
  },
})
