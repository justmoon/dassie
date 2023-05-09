import { extend } from "../../utils/class-helper"

const Card = extend(
  "Card",
  "div",
  "rounded-lg border bg-card text-card-foreground shadow-sm"
)

const CardHeader = extend("CardHeader", "div", "flex flex-col space-y-1.5 p-6")

const CardTitle = extend(
  "CardTitle",
  "h3",
  "text-lg font-semibold leading-none tracking-tight"
)

const CardDescription = extend(
  "CardDescription",
  "p",
  "text-sm text-muted-foreground"
)

const CardContent = extend("CardContent", "div", "p-6 pt-0")

const CardFooter = extend("CardFooter", "div", "flex items-center p-6 pt-0")

export { Card, CardHeader, CardFooter, CardTitle, CardDescription, CardContent }
