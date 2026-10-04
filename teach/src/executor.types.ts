/**
 * Shared interfaces for the AgentMesh Teach workflow executor.
 *
 * The Workflow IR shapes mirror the Rust schema in
 * `crates/agentmesh-teach`; the runtime shapes describe target resolution.
 */

import type { Page } from "playwright-core";

export interface IrInput {
  type: string;
  required?: boolean;
  default?: unknown;
}

export interface IrTarget {
  semantic?: string;
  role?: string;
  accessible_name?: string;
  text?: string;
  placeholder?: string;
  autocomplete?: string;
  selectors?: string[];
  match?: string;
}

export interface IrStep {
  id: string;
  op: string;
  target?: IrTarget;
  value?: string;
  url?: string;
  limit?: string;
  timeout_ms?: number;
  condition?: string;
  iterations?: string;
  destination?: IrTarget;
  path?: string;
}

export interface IrWorkflow {
  version: string;
  id: string;
  description?: string;
  runtime?: string;
  inputs?: Record<string, IrInput>;
  steps: IrStep[];
  outputs?: Record<string, { from: string }>;
  policy?: {
    allowed_origins?: string[];
    allowed_operations?: string[];
    denied_operations?: string[];
    max_runs_per_hour?: number;
  };
  preconditions?: IrAssertion[];
  success?: IrAssertion[];
  failure?: IrAssertion[];
  recovery?: {
    max_attempts?: number;
    checkpoints?: string[];
    capture_aria?: boolean;
    capture_screenshot?: boolean;
    vision_adapter?: string;
  };
}

export interface IrAssertion {
  op: "assert.exists" | "assert.not_exists" | "assert.text" | "assert.url";
  target?: IrTarget;
  value?: string;
  url?: string;
  timeout_ms?: number;
}

export interface Resolution {
  strategy: string;
  confidence: number;
  describe: string;
}

export interface StepOutcome {
  resolution?: Resolution;
  /** A different tab became active (new_tab, close_tab, app.open/close). */
  page?: Page;
}
