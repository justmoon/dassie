import { Link } from "solid-app-router"
import type { Component } from "solid-js"

const MainNavigation: Component = () => {
  return (
    <div class="flex flex-col bg-gray-800 pt-5 inset-y-0 w-64 fixed">
      <div class="font-black flex-shrink-0 px-4 text-2xl overflow-y-auto">
        Xen<span class="text-lime-500">//dev</span>
      </div>
      <nav class="space-y-1 bg-gray-800 flex-1 mt-5 px-2" aria-label="Sidebar">
        <Link
          href="/"
          class="rounded-md flex font-medium bg-gray-900 text-white text-sm py-2 px-2 group items-center"
        >
          <div class="flex-shrink-0 mr-2 text-2xl i-mdi-home" />
          <span class="flex-1 text-base">Dashboard</span>
        </Link>
        <Link
          href="/logs"
          class="rounded-md flex font-medium bg-gray-900 text-white text-sm py-2 px-2 group items-center"
        >
          <div class="flex-shrink-0 mr-2 text-2xl i-mdi-view-list" />
          <span class="flex-1 text-base">Logs</span>
        </Link>
      </nav>
    </div>
  )
}

export default MainNavigation
