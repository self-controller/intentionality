/** The wire shapes. Python is the authority for every string here: the
 *  greeting, the per-row detail line and every refusal are
 *  computed in gate/ui.py and travel as finished text, so there is exactly
 *  one implementation of each. */

export type Mark = "done" | "delete" | null;

export interface Row {
  index: number;
  title: string;
  mark: Mark;
  /** Ticked for today: goes to To Do on Start. Unticked stays in the backlog. */
  today: boolean;
  /** Starts in Doing instead of To Do. Python keeps it on ticked, unmarked
   *  rows only: turning it on ticks the row, unticking or marking clears it.
   *  Absent from a gate older than this bundle, which reads as To Do. */
  doing?: boolean;
  /** ui.row_detail(), or the mark's word when the row is marked. */
  detail: string;
  /** The due date alone, in ui.due_label's words ("due today", "overdue, was
   *  due Sep 18"); "" when there is none or the row is marked. The screen lays
   *  it out itself, in colour when the words say overdue or today. */
  due: string;
  /** Whether the task carries notes. */
  notes: boolean;
  /** Only a task carried from an earlier session can be marked done. */
  can_finish: boolean;
  /** Typed on this screen rather than pulled from the store. */
  typed: boolean;
  /** The row's labels as they stand now; the screen groups rows by these. */
  labels: string[];
}

export interface LabelOpt {
  name: string;
  color: string | null;
  /** False for a label invented on this screen that no task wears yet. */
  exists: boolean;
}

export interface Panel {
  index: number;
  /** Bumped by Python only when React should reseed the fields (panel opened,
   *  panel saved). Used as the panel's React key, so an unrelated state push
   *  cannot clobber what is being typed. */
  token: number;
  notes: string;
  due: string;
  labels: string[];
  error: string;
}

export interface Welcome {
  screen: "welcome";
  greeting: string;
  date: string;
  notes: string;
  error: string;
  rows: Row[];
  labels: LabelOpt[];
  panel: Panel | null;
  minutes: string;
  start_enabled: boolean;
  /** Bumped each time the add box's task lands, and only then: the box is
   *  emptied on that, so a refused add keeps everything that was typed. */
  added: number;
  /** Why the add box's last task did not land ("" when silent or none). */
  add_error: string;
  /** Shortcuts changed in the app's Settings, action id -> combo
   *  (app/src/keybinds.ts). Absent from a gate older than this bundle. */
  keys?: Record<string, string>;
}

export interface Blank {
  screen: "blank";
  log: string;
}

export interface Choice {
  screen: "choice";
  question: string;
  choices: { key: string; label: string }[];
  log: string;
}

export type State = Welcome | Blank | Choice;

/** The uncommitted contents of a details editor: an open row panel, or the
 *  add box's. Lives in Welcome so Start can commit both in the same message
 *  as everything else. */
export interface PanelDraft {
  due: string;
  notes: string;
  labels: string[];
  /** A label typed but not yet turned into a chip; it still counts on save. */
  pending: string;
}
