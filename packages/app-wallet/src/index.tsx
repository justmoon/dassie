/* @refresh reload */
import { render } from "solid-js/web"

import App from "./App"
import "./index.css"
import Uplink from "./modules/uplink"

render(() => <App />, document.getElementById("root") as HTMLElement)

new Uplink()
