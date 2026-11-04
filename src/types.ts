export interface Field {
  key: string;
  value: string;
  category: string;
  retained: boolean;
}
export interface Inspection {
  id: string;
  name: string;
  path: string;
  format: string;
  bytes: number;
  width: number;
  height: number;
  preview: string;
  fields: Field[];
  chunks: string[];
}
export interface ImportIssue {
  path: string;
  message: string;
}
export interface ImportResult {
  images: Inspection[];
  issues: ImportIssue[];
}
export interface ExportResult {
  id: string;
  status: 'verified' | 'cancelled' | 'error';
  output: string | null;
  error: string | null;
  removed: Field[];
  retained: Field[];
  after: Field[];
  removedChunks: string[];
  bytes: number;
  encodedVerified: boolean;
  pixelsVerified: boolean;
}
export interface Progress {
  job: string;
  id: string;
  phase: string;
  completed: number;
  total: number;
  result: ExportResult | null;
}
export interface Health {
  version: string;
  exiftool: string;
  offline: boolean;
}
export interface Preferences {
  destination: string | null;
  isDefault: boolean;
  autoCleanOnDrop: boolean;
  error: string | null;
}
