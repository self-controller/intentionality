/** The wire shapes. Python is the authority for every string here: the
 *  greeting, the per-row detail line, the status line and every refusal are
 *  computed in gate/ui.py and travel as finished text, so there is exactly
 *  one implementation of each. */

export type Mark = "done" | "delete" | null;

export interface Row {
  index: number;
  title: string;
  mark: Mark;
  /** ui.row_detail(), or the mark's word when the row is marked. */
  detail: string;
  /** Only a task carried from an earlier session can be marked done. */
  can_finish: boolean;
  /** Typed on this screen rather than pulled from the store. */
  typed: boolean;
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
  status: string;
  start_enabled: boolean;
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

/** The uncommitted contents of an open details panel. Lives in Welcome so
 *  Start can commit it in the same message as everything else. */
export interface PanelDraft {
  due: string;
  notes: string;
  labels: string[];
  /** A label typed but not yet turned into a chip; it still counts on save. */
  pending: string;
}
