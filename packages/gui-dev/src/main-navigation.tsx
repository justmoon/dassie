import { ClapperboardIcon, HomeIcon, ListIcon } from "lucide-react"
import { Link, useRoute } from "wouter"

import { Button } from "@dassie/gui-dassie/src/components/ui/button"
import { combine } from "@dassie/gui-dassie/src/utils/class-helper"

import NodesList from "./components/nodes-list/nodes-list"

interface NavigationItemProperties {
  children: React.ReactNode
  href: string
  className?: string | undefined
}

const NavigationItem = ({
  children,
  href,
  className,
}: NavigationItemProperties) => {
  const [isActive] = useRoute(href)

  return (
    <Button
      variant={isActive ? "secondary" : "ghost"}
      className={combine("w-full justify-start", className)}
      asChild
    >
      <Link href={href}>{children}</Link>
    </Button>
  )
}

const MainNavigation = () => {
  return (
    <div className="space-y-4 py-4 w-64">
      <div className="mb-2 px-4 text-lg font-semibold tracking-tight">
        Dassie<span className="text-lime-500">{"//dev"}</span>
      </div>
      <nav className="space-y-2 flex-1 px-3" aria-label="Sidebar">
        <NavigationItem href="/">
          <HomeIcon className="mr-2 h-4 w-4" />
          <span className="">Dashboard</span>
        </NavigationItem>
        <NavigationItem href="/logs">
          <ListIcon className="mr-2 h-4 w-4" />
          <span>Logs</span>
        </NavigationItem>
        <NavigationItem href="/scenarios">
          <ClapperboardIcon className="mr-2 h-4 w-4" />
          <span>Scenarios</span>
        </NavigationItem>
      </nav>
      <NodesList />
    </div>
  )
}

export default MainNavigation
