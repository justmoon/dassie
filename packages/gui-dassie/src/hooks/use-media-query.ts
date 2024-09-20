import { useEffect, useState } from "react"

const runQuery = (query: string): boolean => {
  return window.matchMedia(query).matches
}

export const useMediaQuery = (query: string): boolean => {
  const [matches, setMatches] = useState<boolean>(runQuery(query))

  useEffect(() => {
    const matchMedia = window.matchMedia(query)

    function updateState() {
      setMatches(runQuery(query))
    }

    updateState()

    matchMedia.addEventListener("change", updateState)

    return () => {
      matchMedia.removeEventListener("change", updateState)
    }
  }, [query])

  return matches
}
