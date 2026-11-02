import { useCallback, useEffect, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { ChartNoAxesColumn, ScanEye, Settings as SettingsIcon, SquareKanban } from "lucide-react";
import { SegmentList, SegmentTrigger } from "@/components/SegmentTabs";
import { Button } from "@/components/ui/button";
import { ScrollArea, ScrollBar } from "@/components/ui/scroll-area";
import { Tabs, TabsContent } from "@/components/ui/tabs";
import * as api from "./api";
import type { Health } from "./types";
import Analyses from "./Analyses";
import Appeal from "./Appeal";
import Board from "./Board";
import Checkpoint from "./Checkpoint";
import Dashboard from "./Dashboard";
import Settings from "./Settings";
import { matches } from "./keybinds";
import { loadKeybinds, useKeybinds } from "./useKeybinds";
import type { Analysis, PageRef } from "./types";

type Screen = "board" | "analyses" | "dashboard" | "settings";
/** The tabs in header order, for Ctrl+Tab / Ctrl+Shift+Tab. Settings, behind
 *  the gear at the top right, is not one of them: stepping from it counts
 *  from just past the last tab, so forward lands on the Board and back on the
 *  Dashboard. */
const SCREENS: Screen[] = ["board", "analyses", "dashboard"];

/** Something in the header worth a glance: amber, with a dot. */
function Warning({ children }: { children: React.ReactNode }) {
  return (
    <span className="inline-flex items-center gap-1.5 rounded-full border border-warn/20 bg-warn/10 px-2.5 py-0.5 text-xs text-warn">
      <span className="size-1.5 flex-none rounded-full bg-warn" />
      {children}
    </span>
  );
}

function Code({ children }: { children: React.ReactNode }) {
  return (
    <code className="rounded border border-border bg-secondary px-1.5 py-0.5 font-mono text-[0.85em] text-foreground">
      {children}
    </code>
  );
}

/** The whole window when there is no app to show yet: one card on the grid. */
function Splash({ children }: { children: React.ReactNode }) {
  return (
    <div className="bg-grid flex min-h-screen items-center justify-center p-12">
      <div className="surface max-w-md px-8 py-7 text-center text-muted-foreground">{children}</div>
    </div>
  );
}

export default function App() {
  const [healthState, setHealth] = useState<Health | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [screen, setScreen] = useState<Screen>("board");
  const [unseen, setUnseen] = useState(0);
  const [checkpoint, setCheckpoint] = useState<Analysis | null>(null);
  // The page an Analyses row is being appealed for, while its appeal is open.
  // (The on-screen clock argues its own case, in the box under it.)
  const [appeal, setAppeal] = useState<PageRef | null>(null);
  // Ctrl+N requests, counted: the Board acts on each new value once.
  const [newTask, setNewTask] = useState(0);
  // Stable: Analyses' load effect depends on it, and a fresh function every
  // App render would tear down its listener and refetch its whole list.
  const onSeen = useCallback(() => setUnseen((n) => Math.max(0, n - 1)), []);
  const keys = useKeybinds();

  useEffect(loadKeybinds, []);

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

  // Next / previous tab (Ctrl+Tab / Ctrl+Shift+Tab unless Settings says
  // otherwise) step through the tabs, wrapping at either end; New task
  // (Ctrl+N) makes a new task on the board, from any tab (the Board holds
  // the card for the arrow keys once it is saved). None of them while a
  // checkpoint or an appeal is over the window.
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.defaultPrevented || checkpoint || appeal) return;
      if (matches(e, keys["app.newTask"])) {
        e.preventDefault();
        setScreen("board");
        setNewTask((n) => n + 1);
        return;
      }
      const step = matches(e, keys["app.nextTab"]) ? 1 : matches(e, keys["app.prevTab"]) ? -1 : 0;
      if (!step) return;
      e.preventDefault();
      setScreen((s) => {
        const at = s === "settings" ? (step > 0 ? SCREENS.length - 1 : 0) : SCREENS.indexOf(s);
        return SCREENS[(at + step + SCREENS.length) % SCREENS.length];
      });
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [checkpoint, appeal, keys]);

  if (error) return <Splash>{error}</Splash>;
  if (!healthState)
    return <div className="flex min-h-screen items-center justify-center text-muted-foreground">…</div>;
  const build = (
    <>
      v{healthState.build.version}
      {healthState.build.built_at ? ` · built ${healthState.build.built_at}` : ""}
    </>
  );
  if (healthState.needs_migration)
    return (
      <Splash>
        <h2 className="mb-3 text-foreground">The store needs migrating</h2>
        Its schema is out of date. Run <Code>python3 -m gate migrate</Code> in the repo, then reopen this app.
        <p className="mt-5 font-mono text-[11px]">{build}</p>
      </Splash>
    );

  return (
    // Radix renders only the active TabsContent, so a tab is unmounted when
    // it is left and mounted fresh when it is chosen again -- as the old
    // conditional rendering did, which the screens rely on.
    <Tabs value={screen} onValueChange={(v) => setScreen(v as Screen)} className="flex min-h-screen flex-col">
      {/* The window is undecorated, so this header is its drag handle. */}
      <header
        data-tauri-drag-region
        className="sticky top-0 z-20 flex h-14 flex-none items-center gap-5 rounded-t-xl border-b border-border bg-background/80 px-5 backdrop-blur"
      >
        <ScrollArea className="min-w-0">
          <SegmentList>
            <SegmentTrigger value="board" icon={SquareKanban}>
              Board
            </SegmentTrigger>
            <SegmentTrigger value="analyses" icon={ScanEye}>
              Analyses
              {unseen > 0 && (
                <span className="ms-1.5 rounded-full bg-primary px-1.5 font-mono text-[10px] leading-4 text-primary-foreground">
                  {unseen}
                </span>
              )}
            </SegmentTrigger>
            <SegmentTrigger value="dashboard" icon={ChartNoAxesColumn}>
              Dashboard
            </SegmentTrigger>
          </SegmentList>
          <ScrollBar orientation="horizontal" />
        </ScrollArea>
        <div className="ms-auto flex flex-none items-center gap-2">
          {!healthState.aw_ok && <Warning>ActivityWatch unreachable</Warning>}
          {healthState.build.stale_since && (
            <Warning>
              build is behind your source — run <Code>npm run tauri build</Code>
            </Warning>
          )}
          {healthState.resume_gate && <Warning>{healthState.resume_gate}</Warning>}
          {healthState.window_tracking && <Warning>{healthState.window_tracking}</Warning>}
          {healthState.overlay && <Warning>{healthState.overlay}</Warning>}
          {healthState.relevance && <Warning>{healthState.relevance}</Warning>}
          <Button
            variant="ghost"
            mode="icon"
            size="sm"
            aria-label="Settings"
            title="Settings"
            selected={screen === "settings"}
            onClick={() => setScreen("settings")}
          >
            <SettingsIcon />
          </Button>
        </div>
      </header>
      <TabsContent value="board" className="tab-in mt-0 flex flex-1 flex-col">
        <Board newTask={newTask} />
      </TabsContent>
      <TabsContent value="analyses" className="tab-in mt-0">
        <Analyses
          hasSession={healthState.session != null}
          sessionId={healthState.session?.id ?? null}
          appealOpen={appeal != null}
          onAppeal={setAppeal}
          onSeen={onSeen}
        />
      </TabsContent>
      <TabsContent value="dashboard" className="tab-in mt-0">
        {/* The build line is at the foot of the Dashboard; a build behind
            its source still warns up in the header. */}
        <Dashboard build={build} />
      </TabsContent>
      <TabsContent value="settings" className="tab-in mt-0">
        <Settings />
      </TabsContent>
      {appeal && (
        <Appeal key={appeal.app + "\n" + appeal.title} page={appeal} onClose={() => setAppeal(null)} />
      )}
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
    </Tabs>
  );
}
