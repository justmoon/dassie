import { Link } from "wouter"

const MainNavigation = () => {
  return (
    <div className="flex flex-col bg-gray-800 pt-5 inset-y-0 w-64 fixed">
      <div className="font-black flex-shrink-0 px-4 text-2xl overflow-y-auto">
        Xen<span className="text-lime-500">//dev</span>
      </div>
      <nav
        className="space-y-1 bg-gray-800 flex-1 mt-5 px-2"
        aria-label="Sidebar"
      >
        <Link
          href="/"
          className="rounded-md flex font-medium bg-gray-900 text-white text-sm py-2 px-2 group items-center"
        >
          <div className="flex-shrink-0 mr-2 text-2xl i-mdi-home" />
          <span className="flex-1 text-base">Dashboard</span>
        </Link>
        <Link
          href="/logs"
          className="rounded-md flex font-medium bg-gray-900 text-white text-sm py-2 px-2 group items-center"
        >
          <div className="flex-shrink-0 mr-2 text-2xl i-mdi-view-list" />
          <span className="flex-1 text-base">Logs</span>
        </Link>
      </nav>
    </div>
  )
}

export default MainNavigation
