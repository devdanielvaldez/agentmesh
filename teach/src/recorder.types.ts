/**
 * Shared interfaces for the AgentMesh Teach browser recorder.
 */

export interface Args {
  profile: string;
  trace: string;
  scope: string;
  startUrl: string;
  headed: boolean;
  smoke: boolean;
  verbose: boolean;
  cdpPort: number;
}

export interface InPageTarget {
  tag: string;
  role: string;
  name: string;
  placeholder: string;
  testId: string;
  id: string;
  text: string;
  inputType: string;
  autocomplete: string;
  value: string;
  secret: boolean;
  selectors: string[];
}
