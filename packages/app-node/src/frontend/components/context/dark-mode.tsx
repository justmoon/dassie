import {
  type ReactNode,
  createContext,
  useContext,
  useEffect,
  useState,
} from "react"

import { useMediaQuery } from "../../hooks/use-media-query"

export type DarkModeSetting = "dark" | "light" | "system"

export interface DarkModeContextProperties {
  darkMode: "dark" | "light"
  setDarkMode: (darkMode: DarkModeSetting) => void
}

const DarkModeContext = createContext<DarkModeContextProperties>({
  darkMode: "light",
  setDarkMode: () => {
    throw new Error("DarkModeContext not initialized")
  },
})

const SYSTEM_DARK_MODE_QUERY = "(prefers-color-scheme: dark)"

export const useDarkMode = () => {
  return useContext(DarkModeContext)
}

export const DarkModeProvider = ({ children }: { children: ReactNode }) => {
  const systemDarkMode =
    useMediaQuery(SYSTEM_DARK_MODE_QUERY) ? "dark" : "light"
  const [darkModeSetting, setDarkModeSetting] =
    useState<DarkModeSetting>("system")

  const darkMode =
    darkModeSetting === "system" ? systemDarkMode : darkModeSetting

  useEffect(() => {
    if (darkMode === "dark") {
      document.documentElement.classList.add("dark")
    } else {
      document.documentElement.classList.remove("dark")
    }
  }, [darkMode])

  return (
    <DarkModeContext.Provider
      value={{
        darkMode,
        setDarkMode: setDarkModeSetting,
      }}
    >
      {children}
    </DarkModeContext.Provider>
  )
}
