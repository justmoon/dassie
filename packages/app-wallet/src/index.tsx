/* @refresh reload */
import { assertDefined } from "@dassie/lib-type-utils"
import { createRoot } from "react-dom/client"
import "virtual:windi.css"

import App from "./app"
import "./index.css"
import Uplink from "./modules/uplink"

const rootElement = document.querySelector("#root")
assertDefined(rootElement)
const root = createRoot(rootElement)
root.render(<App />)

new Uplink()
