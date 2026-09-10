import { useEffect, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import * as api from "./api";
import type { Health } from "./types";
import Analyses from "./Analyses";
import Board from "./Board";
import Checkpoint from "./Checkpoint";
import Dashboard from "./Dashboard";
import Meetings from "./Meetings";
import type { Analysis } from "./types";

type Screen = "board" | "analyses" | "meetings" | "dashboard";

export default function App() {
  const [healthState, setHealth] = useState<Health | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [screen, setScreen] = useState<Screen>("board");
  const [unseen, setUnseen] = useState(0);
  const [checkpoint, setCheckpoint] = useState<Analysis | null>(null);

  useEffect(() => {
    api
      .health()
      .then(setHealth)
      .catch((e) => setError(String(e)));
    // Counted from the same rows the Analyses tab lists, so the badge is right
    // even between sessions — when the old per-session count read zero.
    api
      .listRecentAnalyses(api.RECENT_ANALYSES)
      .then((list) => setUnseen(list.filter((a) => !a.seen_at).length))
      .catch(() => {});
    const unlistenNew = listen("analysis:new", () => setUnseen((n) => n + 1));
    const unlistenClosed = listen("session:closed", () =>
      api.health().then(setHealth).catch(() => {}),
    );
    // The resume gate starts a session on its own console after the machine
    // wakes; this is how the board learns it is no longer between sessions.
    const unlistenOpened = listen("session:opened", () =>
      api.health().then(setHealth).catch(() => {}),
    );
    // Asked for on start as well as listened for: a checkpoint that fired
    // while the app was restarting is still waiting in the store, and losing
    // it would be losing the one thing the user asked to be told.
    const showPending = () =>
      api.pendingCheckpoint().then(setCheckpoint).catch(() => {});
    showPending();
    const unlistenCheckpoint = listen("checkpoint:new", () => {
      showPending();
      setUnseen((n) => n + 1); // it is an unseen analysis row like any other
    });
    return () => {
      unlistenNew.then((f) => f());
      unlistenClosed.then((f) => f());
      unlistenOpened.then((f) => f());
      unlistenCheckpoint.then((f) => f());
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
        <p className="build-stamp">
          v{healthState.build.version}
          {healthState.build.built_at ? ` · built ${healthState.build.built_at}` : ""}
        </p>
      </div>
    );

  return (
    <div className="app">
      <header>
        <nav>
          <button className={screen === "board" ? "active" : ""} onClick={() => setScreen("board")}>
            Board
          </button>
          <button
            className={screen === "analyses" ? "active" : ""}
            onClick={() => setScreen("analyses")}
          >
            Analyses
            {unseen > 0 && <span className="badge">{unseen}</span>}
          </button>
          <button
            className={screen === "meetings" ? "active" : ""}
            onClick={() => setScreen("meetings")}
          >
            Meetings
          </button>
          <button
            className={screen === "dashboard" ? "active" : ""}
            onClick={() => setScreen("dashboard")}
          >
            Dashboard
          </button>
        </nav>
        <div className="header-status">
          {!healthState.aw_ok && <span className="header-warn">ActivityWatch unreachable</span>}
          {healthState.build.stale_since && (
            <span className="header-warn">
              build is behind your source — run <code>npm run tauri build</code>
            </span>
          )}
          {/* Always present, not only when something is wrong: the point is
              that the running build can be identified at a glance. */}
          <span className="build-stamp">
            v{healthState.build.version}
            {healthState.build.built_at ? ` · built ${healthState.build.built_at}` : ""}
          </span>
        </div>
      </header>
      {screen === "board" && <Board />}
      {screen === "analyses" && (
        <Analyses
          hasSession={healthState.session != null}
          onSeen={() => setUnseen((n) => Math.max(0, n - 1))}
        />
      )}
      {screen === "meetings" && <Meetings />}
      {screen === "dashboard" && <Dashboard />}
      {checkpoint && (
        <Checkpoint
          analysis={checkpoint}
          session={healthState.session}
          onDismiss={() => {
            setCheckpoint(null);
            setUnseen((n) => Math.max(0, n - 1)); // dismissing marks it seen
          }}
        />
      )}
    </div>
  );
}
