import { extend } from "../../utils/class-helper"

const Table = extend("Table", "table", "w-full caption-bottom text-sm")

const TableHeader = extend("TableHeader", "thead", "[&_tr]:border-b")

const TableBody = extend("TableBody", "tbody", "[&_tr:last-child]:border-0")

const TableFooter = extend(
  "TableFooter",
  "tfoot",
  "bg-primary font-medium text-primary-foreground"
)

const TableRow = extend(
  "TableRow",
  "tr",
  "border-b transition-colors hover:bg-muted/50 data-[state=selected]:bg-muted"
)

const TableHead = extend(
  "TableHead",
  "th",
  "h-12 px-4 text-left align-middle font-medium text-muted-foreground [&:has([role=checkbox])]:pr-0"
)

const TableCell = extend(
  "TableCell",
  "td",
  "p-4 align-middle [&:has([role=checkbox])]:pr-0"
)

const TableCaption = extend(
  "TableCaption",
  "caption",
  "mt-4 text-sm text-muted-foreground"
)

export {
  Table,
  TableHeader,
  TableBody,
  TableFooter,
  TableHead,
  TableRow,
  TableCell,
  TableCaption,
}
