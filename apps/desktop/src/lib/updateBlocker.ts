// Why "Install and restart" must wait: the update restarts Diarist, which
// would end a recording, an import, a post-pass or a punctuation job. The same rule and words
// as `install_update` (commands.rs), with the jobs named. Free of Svelte and
// Tauri so `bun test` can run it (tests/updateBlocker.test.ts).

/** What the job list knows about a job (`Job` in state.svelte.ts). */
export interface JobLike {
  kind: "import" | "postpass" | "punctuate";
  name: string;
  /** `queued` until it starts. */
  stage: string;
}

function describe(j: JobLike): string {
  if (j.kind === "punctuate") return "A punctuation job";
  return j.kind === "postpass" ? `Finalizing "${j.name}"` : `The import of ${j.name}`;
}

/** The reason, or null when the update can start. */
export function updateBlocker(recording: boolean, jobs: readonly JobLike[]): string | null {
  if (recording) return "Stop the recording before updating.";
  const first = jobs[0];
  if (!first) return null;
  const running = jobs.find((j) => j.stage !== "queued");
  const waiting = jobs.length - (running ? 1 : 0);
  const restart = "and updating restarts Diarist.";
  if (running && waiting === 0) {
    return `${describe(running)} is still running, ${restart} Wait for it to finish or cancel it, then update.`;
  }
  if (running) {
    const more = waiting === 1 ? "1 more job waits" : `${waiting} more jobs wait`;
    return `${describe(running)} is still running and ${more} in the queue, ${restart} Wait for them to finish or cancel them, then update.`;
  }
  return waiting === 1
    ? `${describe(first)} waits in the queue, ${restart} Wait for it to finish or cancel it, then update.`
    : `${waiting} jobs wait in the queue, ${restart} Wait for them to finish or cancel them, then update.`;
}
