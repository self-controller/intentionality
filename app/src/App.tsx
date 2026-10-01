import { useCallback, useEffect, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { ChartNoAxesColumn, Mic, ScanEye, SquareKanban } from "lucide-react";
import { SegmentList, SegmentTrigger } from "@/components/SegmentTabs";
import { ScrollArea, ScrollBar } from "@/components/ui/scroll-area";
import { Tabs, TabsContent } from "@/components/ui/tabs";
import * as api from "./api";
import type { Health } from "./types";
import Analyses from "./Analyses";
import Board from "./Board";
import Checkpoint from "./Checkpoint";
import Dashboard from "./Dashboard";
import Meetings from "./Meetings";
import type { Analysis } from "./types";

type Screen = "board" | "analyses" | "meetings" | "dashboard";

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
      <header className="sticky top-0 z-20 flex h-14 flex-none items-center gap-5 border-b border-border bg-background/80 px-5 backdrop-blur">
        <span className="flex-none select-none text-lg font-semibold tracking-[-0.04em]">intentionality</span>
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
            <SegmentTrigger value="meetings" icon={Mic}>
              Meetings
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
          {/* Always present, not only when something is wrong: the point is
              that the running build can be identified at a glance. */}
          <span className="ms-1 font-mono text-[11px] text-muted-foreground">{build}</span>
        </div>
      </header>
      <TabsContent value="board" className="tab-in mt-0 flex flex-1 flex-col">
        <Board />
      </TabsContent>
      <TabsContent value="analyses" className="tab-in mt-0">
        <Analyses hasSession={healthState.session != null} onSeen={onSeen} />
      </TabsContent>
      <TabsContent value="meetings" className="tab-in mt-0">
        <Meetings />
      </TabsContent>
      <TabsContent value="dashboard" className="tab-in mt-0">
        <Dashboard />
      </TabsContent>
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
