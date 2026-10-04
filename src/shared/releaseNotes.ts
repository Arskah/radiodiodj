/**
 * Reduce a release's Markdown notes to text worth reading in a plain box.
 * The notes are release-please's changelog section: mostly links to commits
 * and pull requests, which say nothing once they cannot be followed.
 * @param markdown The release body.
 * @returns The notes with links flattened and commit references dropped.
 */
export function plainNotes(markdown: string): string {
  return (
    markdown
      // "([abc1234](…/commit/…))" — a hash an operator has no use for.
      .replace(/\s*\(\[[0-9a-f]{7,40}\]\([^)]*\)\)/g, "")
      .replace(/\[([^\]]+)\]\([^)]*\)/g, "$1")
      .replace(/^#{1,6}\s+/gm, "")
      .replace(/^\* /gm, "• ")
      .replace(/\*\*([^*]+)\*\*/g, "$1")
      .replace(/\n{3,}/g, "\n\n")
      .trim()
  );
}
