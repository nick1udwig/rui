export class DocumentView {
  constructor(container: HTMLElement, api: (path: string, data: unknown, authenticated?: boolean, signal?: AbortSignal) => Promise<any>, dispatch: (command: unknown) => void, canControl: () => boolean, report: (error: unknown) => void, onRendered?: () => void);
  readonly soundButton: HTMLButtonElement;
  readonly generation: string | null;
  diagnostics(): Record<string, unknown>;
  start(): void;
  stop(): void;
}
