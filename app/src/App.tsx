import { useCallback, useEffect, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import * as api from "./api";
import type { Health } from "./types";
import Analyses from "./Analyses";
import Board from "./Board";
import Checkpoint from "./Checkpoint";
import { Tab } from "./ui/primitives";
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
  // Stable: Analyses' load effect depends on it, and a fresh function every
  // App render would tear down its listener and refetch its whole list.
  const onSeen = useCallback(() => setUnseen((n) => Math.max(0, n - 1)), []);

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
    // wakes; this is how the header learns it is no longer between sessions.
    // Board and Dashboard listen for themselves.
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

  if (error) return <div className="p-12 text-center text-muted">{error}</div>;
  if (!healthState) return <div className="p-12 text-center text-muted">…</div>;
  if (healthState.needs_migration)
    return (
      <div className="p-12 text-center text-muted">
        The store's schema is out of date.
        <br />
        Run <code className="rounded bg-raised px-1 py-0.5 font-mono text-xs">python3 -m gate migrate</code> in the repo, then reopen this app.
        <p className="mt-4 text-xs text-muted">
          v{healthState.build.version}
          {healthState.build.built_at ? ` · built ${healthState.build.built_at}` : ""}
        </p>
      </div>
    );

  return (
    <div className="flex min-h-screen flex-col">
      <header className="flex items-center justify-between border-b border-line px-4 py-2.5">
        <nav className="flex gap-2">
          <Tab active={screen === "board"} onClick={() => setScreen("board")}>
            Board
          </Tab>
          <Tab active={screen === "analyses"} onClick={() => setScreen("analyses")}>
            Analyses
            {unseen > 0 && <span className="ml-1.5 rounded-md bg-accent px-1.5 text-xs text-bg">{unseen}</span>}
          </Tab>
          <Tab active={screen === "meetings"} onClick={() => setScreen("meetings")}>
            Meetings
          </Tab>
          <Tab active={screen === "dashboard"} onClick={() => setScreen("dashboard")}>
            Dashboard
          </Tab>
        </nav>
        <div className="flex items-center gap-3">
          {!healthState.aw_ok && <span className="text-xs text-warn">ActivityWatch unreachable</span>}
          {healthState.build.stale_since && (
            <span className="text-xs text-warn">
              build is behind your source — run <code className="rounded bg-raised px-1 py-0.5 font-mono text-xs">npm run tauri build</code>
            </span>
          )}
          {healthState.resume_gate && <span className="text-xs text-warn">{healthState.resume_gate}</span>}
          {/* Always present, not only when something is wrong: the point is
              that the running build can be identified at a glance. */}
          <span className="text-xs text-muted">
            v{healthState.build.version}
            {healthState.build.built_at ? ` · built ${healthState.build.built_at}` : ""}
          </span>
        </div>
      </header>
      {screen === "board" && <Board />}
      {screen === "analyses" && (
        <Analyses
          hasSession={healthState.session != null}
          onSeen={onSeen}
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
