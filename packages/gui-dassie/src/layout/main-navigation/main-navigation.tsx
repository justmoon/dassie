import { SettingsIcon } from "lucide-react"
import type { ComponentPropsWithoutRef } from "react"
import { Link } from "wouter"

import {
  NavigationMenu,
  NavigationMenuContent,
  NavigationMenuItem,
  NavigationMenuLink,
  NavigationMenuList,
  NavigationMenuTrigger,
  navigationMenuTriggerStyle,
} from "../../components/ui/navigation-menu"
import { combine } from "../../utils/class-helper"
import { NetworkStatus } from "./network-status"

export function MainNavigation() {
  return (
    <NavigationMenu className="p-2">
      <NavigationMenuList>
        <NavigationMenuItem>
          <Link href="/dashboard" asChild>
            <NavigationMenuLink className={navigationMenuTriggerStyle()}>
              Dashboard
            </NavigationMenuLink>
          </Link>
        </NavigationMenuItem>
        <NavigationMenuItem>
          <Link href="/network" asChild>
            <NavigationMenuLink className={navigationMenuTriggerStyle()}>
              Network
            </NavigationMenuLink>
          </Link>
        </NavigationMenuItem>
        <NavigationMenuItem>
          <Link href="/ledgers" asChild>
            <NavigationMenuLink className={navigationMenuTriggerStyle()}>
              Ledgers
            </NavigationMenuLink>
          </Link>
        </NavigationMenuItem>
        <NavigationMenuItem>
          <NavigationMenuTrigger>Debug</NavigationMenuTrigger>
          <NavigationMenuContent>
            <ul className="grid gap-3 p-6 md:w-[400px] lg:w-[500px]">
              <ListItem href="/debug/logs" title="Logs" />
              <ListItem href="/debug/nodes" title="Nodes" />
              <ListItem href="/debug/ledger" title="Ledger" />
              <ListItem href="/debug/routing" title="Routing" />
            </ul>
          </NavigationMenuContent>
        </NavigationMenuItem>
      </NavigationMenuList>
      <div className="flex-1" />
      <NavigationMenuList>
        <NavigationMenuItem>
          <Link href="/debug/nodes" asChild>
            <NavigationMenuLink className={navigationMenuTriggerStyle()}>
              <NetworkStatus />
            </NavigationMenuLink>
          </Link>
        </NavigationMenuItem>
        <NavigationMenuItem>
          <Link href="/settings" asChild>
            <NavigationMenuLink className={navigationMenuTriggerStyle()}>
              <SettingsIcon className="w-4 h-4 md:mr-2 inline-block" />
              <span className="hidden md:inline">Settings</span>
            </NavigationMenuLink>
          </Link>
        </NavigationMenuItem>
      </NavigationMenuList>
    </NavigationMenu>
  )
}

const ListItem = ({
  href,
  className,
  title,
  children,
  ...properties
}: { href: string } & ComponentPropsWithoutRef<"a">) => {
  return (
    <li>
      <NavigationMenuLink asChild>
        <Link href={href} asChild>
          <a
            title={title}
            className={combine(
              "block select-none space-y-1 rounded-md p-3 leading-none no-underline outline-none transition-colors hover:bg-accent hover:text-accent-foreground focus:bg-accent focus:text-accent-foreground",
              className,
            )}
            {...properties}
          >
            <div className="text-sm font-medium leading-none">{title}</div>
            <p className="line-clamp-2 text-sm leading-snug text-muted-foreground">
              {children}
            </p>
          </a>
        </Link>
      </NavigationMenuLink>
    </li>
  )
}
ListItem.displayName = "ListItem"
