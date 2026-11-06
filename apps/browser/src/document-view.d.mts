export class DocumentView {
  constructor(container: HTMLElement, api: (path: string, data: unknown, authenticated?: boolean) => Promise<any>, dispatch: (command: unknown) => void, canControl: () => boolean, report: (error: unknown) => void, onRendered?: () => void);
  readonly generation: string | null;
  start(): void;
  stop(): void;
}
