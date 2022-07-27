import { withSolid } from "react-solid-state"

import LogViewer from "../log-viewer/log-viewer"

const Logs = withSolid(() => {
  return () => (
    <div className="flex flex-col h-full max-h-screen min-h-0 py-10">
      <header>
        <div className="mx-auto max-w-7xl px-4 sm:px-6 lg:px-8">
          <h1 className="font-bold leading-tight text-3xl text-gray-100">
            Logs
          </h1>
        </div>
      </header>
      <main className="rounded-lg flex flex-col mx-auto bg-gray-800 flex-1 mt-4 min-h-0 w-full max-w-7xl p-4 col-span-2">
        <LogViewer />
      </main>
    </div>
  )
})

export default Logs
