import { QUERY_PARAMETER_DEVELOPMENT_SESSION } from "../../common/constants/ui-query-parameter-names"

export const checkForDevelopmentSessionToken = async () => {
  const { search, protocol, host, pathname } = window.location
  const urlParameters = new URLSearchParams(search)
  const token = urlParameters.get(QUERY_PARAMETER_DEVELOPMENT_SESSION)

  if (token) {
    const response = await fetch("/api/_dev/set-session", {
      method: "POST",
      headers: {
        "Content-Type": "application/json",
      },
      body: JSON.stringify({
        sessionToken: token,
      }),
    })

    if (!response.ok) {
      throw new Error(
        `Setting development session token failed with status ${response.status}`
      )
    }

    urlParameters.delete(QUERY_PARAMETER_DEVELOPMENT_SESSION)

    const queryString = urlParameters.toString()
    const newUrl = `${protocol}//${host}${pathname}${
      queryString ? "?" + queryString : ""
    }`

    window.history.replaceState({}, "", newUrl)
    window.location.reload()
  }
}
