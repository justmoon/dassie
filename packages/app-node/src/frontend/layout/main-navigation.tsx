import { Link, useRoute } from "wouter"

import { combine } from "../utils/class-helper"

interface NavigationLinkProperties {
  href: string
  label: string
}

function NavigationLink({ href, label }: NavigationLinkProperties) {
  const [isActive] = useRoute(href)
  return (
    <Link
      href={href}
      className={combine(
        "text-sm font-medium transition-colors hover:text-primary",
        { "text-muted-foreground": !isActive }
      )}
    >
      {label}
    </Link>
  )
}

export function MainNavigation({
  className,
  ...properties
}: React.HTMLAttributes<HTMLElement>) {
  const links = [
    {
      href: "/dashboard",
      label: "Dashboard",
    },
    {
      href: "/debug/ledger",
      label: "Debug",
    },
  ].map(({ href, label }) => (
    <NavigationLink key={href} href={href} label={label} />
  ))

  return (
    <nav
      className={combine("flex items-center space-x-4 lg:space-x-6", className)}
      {...properties}
    >
      {links}
    </nav>
  )
}
