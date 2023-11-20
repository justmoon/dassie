import { Client } from "xrpl"

import assert from "node:assert"

import { isRippledErrorWithId } from "../utils/is-rippled-error"

export const getTransaction = async (
  client: Client,
  transactionHash: string,
) => {
  try {
    const result = await client.request({
      command: "tx",
      transaction: transactionHash,
    })

    const meta = result.result.meta

    assert(
      typeof meta === "object",
      "Expected meta to be an object since we did not pass binary=true option",
    )

    return { ...result.result, meta }
  } catch (error) {
    if (isRippledErrorWithId(error, "txnNotFound")) {
      return false
    } else {
      throw error
    }
  }
}
