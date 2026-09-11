// Polls the agent's braid state and renders it. Degrades rather than blanking:
// when the agent is unreachable the last good state stays on screen with a note.

const FIELDS = [
  ["generation", "generation"],
  ["session_id", "session"],
  ["open_missions", "open missions"],
  ["last_promotion_ns", "last promotion ns"],
  ["last_quarantine_ns", "last quarantine ns"],
];

const statusEl = document.getElementById("status");
const listEl = document.getElementById("braid");

function render(state) {
  listEl.replaceChildren();
  for (const [key, label] of FIELDS) {
    const dt = document.createElement("dt");
    dt.textContent = label;
    const dd = document.createElement("dd");
    dd.textContent = state[key] ?? "—";
    listEl.append(dt, dd);
  }
}

let lastGood = null;

async function tick() {
  try {
    const res = await fetch("/braid", { headers: { accept: "application/json" } });
    if (!res.ok) throw new Error(`HTTP ${res.status}`);
    const state = await res.json();
    lastGood = state;
    render(state);
    statusEl.classList.remove("error");
    statusEl.textContent = state.schema_version ?? "ok";
  } catch (err) {
    statusEl.classList.add("error");
    statusEl.textContent = lastGood
      ? `agent unreachable (${err.message}) — showing last known state`
      : `agent unreachable (${err.message})`;
  }
}

tick();
setInterval(tick, 1000);
