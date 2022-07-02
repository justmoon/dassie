import type { Component } from "solid-js"

import LogViewer from "../log-viewer/log-viewer"

const Logs: Component = () => {
  return (
    <div class="flex-1 py-10">
      <header>
        <div class="mx-auto max-w-7xl px-4 sm:px-6 lg:px-8">
          <h1 class="font-bold leading-tight text-3xl text-gray-100">Logs</h1>
        </div>
      </header>
      <main>
        <div class="mx-auto max-w-7xl sm:px-6 lg:px-8">
          <div class="py-8 px-4 sm:px-0">
            <LogViewer />
          </div>
        </div>
      </main>
    </div>
  )
}

export default Logs
