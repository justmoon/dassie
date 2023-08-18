import { hexToBytes } from "@noble/hashes/utils"

import { login } from "./authentication"

export const checkForDevelopmentSessionToken = async () => {
  const { search, protocol, host, pathname } = window.location
  const urlParameters = new URLSearchParams(search)
  const token = urlParameters.get("_DASSIE_DEV_SEED")

  if (token) {
    await login(hexToBytes(token))

    urlParameters.delete("_DASSIE_DEV_SEED")

    const queryString = urlParameters.toString()
    const newUrl = `${protocol}//${host}${pathname}${
      queryString ? "?" + queryString : ""
    }`

    window.history.replaceState({}, "", newUrl)
    window.location.reload()
  }
}
