import LogViewer from "../../log-viewer/log-viewer"
import NodeGraph from "../../node-graph/node-graph"

const Dashboard = () => {
  return (
    <div className="h-screen grid grid-rows-[min-content_auto] py-10 gap-4">
      <header>
        <h1 className="font-bold leading-tight text-3xl px-4">Dashboard</h1>
      </header>
      <main className="min-h-0 relative px-4">
        <div className="h-full grid grid-rows-[min-content_auto] grid-cols-1 gap-4">
          <div className="h-lg">
            <NodeGraph />
          </div>
          <div className="min-h-0 col-span-2">
            <LogViewer />
          </div>
        </div>
      </main>
    </div>
  )
}

export default Dashboard
