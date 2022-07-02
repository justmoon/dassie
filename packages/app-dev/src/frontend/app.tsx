import { Route, Routes } from "solid-app-router"
import type { Component } from "solid-js"

import Dashboard from "./components/pages/dashboard"
import Logs from "./components/pages/logs"
import MainNavigation from "./main-navigation"

const App: Component = () => {
  return (
    <div class="flex min-h-full items-stretch">
      <MainNavigation />
      <Routes>
        <Route path="/" component={Dashboard} />
        <Route path="/logs" component={Logs} />
      </Routes>
    </div>
  )
}

export default App
