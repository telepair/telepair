// web/src/lib/format.ts
//
// Display-format helpers shared across the recording pages and dialogs.

const DEFAULT_DATE_OPTIONS: Intl.DateTimeFormatOptions = {
  year: 'numeric',
  month: 'short',
  day: '2-digit',
  hour: '2-digit',
  minute: '2-digit',
};

/** Render an ISO-8601 timestamp in the user's locale. Falls back to the
 *  raw string when the input is unparseable so the UI never shows an
 *  empty field. */
export function formatDate(
  iso: string,
  options: Intl.DateTimeFormatOptions = DEFAULT_DATE_OPTIONS,
): string {
  try {
    return new Date(iso).toLocaleString(undefined, options);
  } catch {
    return iso;
  }
}

/** Render a byte count with kilobyte/megabyte units. */
export function formatBytes(bytes: number): string {
  if (bytes === 0) return '0 B';
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
  return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
}
