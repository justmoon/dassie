import type { Component } from "solid-js"

import LogViewer from "../log-viewer/log-viewer"

const Logs: Component = () => {
  return (
    <div class="flex flex-col h-full max-h-screen min-h-0 py-10">
      <header>
        <div class="mx-auto max-w-7xl px-4 sm:px-6 lg:px-8">
          <h1 class="font-bold leading-tight text-3xl text-gray-100">Logs</h1>
        </div>
      </header>
      <main class="flex flex-col mx-auto flex-1 min-h-0 w-full max-w-7xl pt-8 sm:px-6 lg:px-8">
        <LogViewer />
      </main>
    </div>
  )
}

export default Logs
