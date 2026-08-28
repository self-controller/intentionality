import { useEffect, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import * as api from "./api";
import type { Health } from "./types";
import Board from "./Board";
import Dashboard from "./Dashboard";

type Screen = "board" | "dashboard";

export default function App() {
  const [healthState, setHealth] = useState<Health | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [screen, setScreen] = useState<Screen>("board");
  const [unseen, setUnseen] = useState(0);

  useEffect(() => {
    api
      .health()
      .then((h) => {
        setHealth(h);
        if (h.session) api.getBoard().then((b) => setUnseen(b.unseen)).catch(() => {});
      })
      .catch((e) => setError(String(e)));
    const unlistenNew = listen("analysis:new", () => setUnseen((n) => n + 1));
    const unlistenClosed = listen("session:closed", () =>
      api.health().then(setHealth).catch(() => {}),
    );
    return () => {
      unlistenNew.then((f) => f());
      unlistenClosed.then((f) => f());
    };
  }, []);

  if (error) return <div className="fatal">{error}</div>;
  if (!healthState) return <div className="fatal">…</div>;
  if (healthState.needs_migration)
    return (
      <div className="fatal">
        The store's schema is out of date.
        <br />
        Run <code>python3 -m gate migrate</code> in the repo, then reopen this app.
      </div>
    );

  return (
    <div className="app">
      <header>
        <nav>
          <button className={screen === "board" ? "active" : ""} onClick={() => setScreen("board")}>
            Board
            {unseen > 0 && <span className="badge">{unseen}</span>}
          </button>
          <button
            className={screen === "dashboard" ? "active" : ""}
            onClick={() => setScreen("dashboard")}
          >
            Dashboard
          </button>
        </nav>
        {!healthState.aw_ok && <span className="aw-warn">ActivityWatch unreachable</span>}
      </header>
      {screen === "board" ? <Board onSeen={() => setUnseen(0)} /> : <Dashboard />}
    </div>
  );
}
