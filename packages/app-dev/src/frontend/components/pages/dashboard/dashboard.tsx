import LogViewer from "../../log-viewer/log-viewer"
import NodeGraph from "../../node-graph/node-graph"
import NodesList from "./nodes-list"

const Dashboard = () => {
  return (
    <div className="h-screen grid grid-rows-[min-content_auto] py-10 gap-4">
      <header>
        <div className="mx-auto max-w-7xl px-4 sm:px-6 lg:px-8">
          <h1 className="font-bold leading-tight text-3xl text-gray-100">
            Dashboard
          </h1>
        </div>
      </header>
      <main className="min-h-0 relative">
        <div className="h-full mx-auto min-h-0 max-w-7xl grid grid-rows-[min-content_auto] px-4 gap-4 grid-cols-[300px_auto] sm:px-6 sm:px-0 lg:px-8">
          <NodesList />
          <div className="h-lg">
            <NodeGraph />
          </div>
          <div className="rounded-lg bg-gray-800 min-h-0 p-4 col-span-2">
            <LogViewer />
          </div>
        </div>
      </main>
    </div>
  )
}

export default Dashboard
