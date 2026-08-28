export interface Session {
  id: number;
  started_at: string;
  ended_at: string | null;
  close_reason: string | null;
  statement: string;
  intended_minutes: number | null;
}

export type Status = "planned" | "doing" | "done" | "dropped";

export interface Task {
  id: number;
  session_id: number | null;
  title: string;
  position: number;
  status: Status;
  carry_count: number;
}

export interface Board {
  session: Session | null;
  tasks: Task[];
  backlog: Task[];
  unseen: number;
}

export interface Observed {
  per_app: Record<string, number>;
  active_seconds: number;
  afk_seconds: number;
}

export interface Analysis {
  id: number;
  session_id: number;
  created_at: string;
  window_start: string;
  window_end: string;
  headline: string;
  alignment: number | null;
  body: string;
  seen_at: string | null;
}

export interface Health {
  schema_version: number;
  needs_migration: boolean;
  session: Session | null;
  aw_ok: boolean;
}

export interface Arrangement {
  todo: number[];
  doing: number[];
  done: number[];
  dropped: number[];
}
