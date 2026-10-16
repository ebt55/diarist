// The import sheet: files waiting for their language (`ImportSheet.svelte`).
// `app.importPaths(paths)` opens it; Import calls `app.importPaths(paths, language)`.

class ImportSheetState {
  paths = $state.raw<string[]>([]);
  isOpen = $state(false);
  /** The language chosen last in this session (preselected next time). */
  last = $state<string | null>(null);

  show(paths: string[]): void {
    this.paths = paths;
    this.isOpen = true;
  }

  close(): void {
    this.isOpen = false;
  }
}

export const importSheet = new ImportSheetState();
