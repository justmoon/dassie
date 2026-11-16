// The window the app shows while its node waits for its packets in flight
// before it quits (crates/dassie-desktop/src/quitting.rs). The app tells it
// what to show in the fragment: `count`, the packets left, `until`, when
// the last is answered at the latest, in ms since the epoch, and `cancel`,
// whether the quit can still be cancelled. Its buttons are links the app
// takes.

let until = null

function render() {
  const shown = new URLSearchParams(location.hash.slice(1))
  const count = Number(shown.get("count") ?? 0)
  until = shown.get("until") ? Number(shown.get("until")) : null
  const packets = count === 1 ? "1 packet" : `${count} packets`
  document.getElementById("waiting").dataset.packets = packets
  document.getElementById("cancel").hidden = shown.get("cancel") !== "1"
  tick()
}

function tick() {
  const waiting = document.getElementById("waiting")
  const packets = waiting.dataset.packets ?? "packets"
  let text = `Your node is waiting for the answers to ${packets} still on their way. Dassie quits as soon as they arrive`
  if (until !== null) {
    const seconds = Math.max(0, Math.ceil((until - Date.now()) / 1000))
    text += seconds === 1 ? ", in 1 second at most." : `, in ${seconds} seconds at most.`
  } else {
    text += "."
  }
  waiting.textContent = text
}

addEventListener("hashchange", render)
addEventListener("keydown", (event) => {
  if (event.key === "Escape" && !document.getElementById("cancel").hidden) {
    location.href = "action/cancel"
  }
})
render()
setInterval(tick, 250)
