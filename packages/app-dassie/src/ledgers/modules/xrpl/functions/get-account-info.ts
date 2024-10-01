import { type AccountInfoResponse, Client } from "xrpl"

import { isRippledErrorWithId } from "../utils/is-rippled-error"

export const getAccountInfo = async (
  client: Client,
  address: string,
): Promise<AccountInfoResponse | false> => {
  try {
    const response = await client.request({
      command: "account_info",
      account: address,
      ledger_index: "validated",
    })
    return response
  } catch (error) {
    if (isRippledErrorWithId(error, "actNotFound")) {
      return false
    } else {
      throw error
    }
  }
}
